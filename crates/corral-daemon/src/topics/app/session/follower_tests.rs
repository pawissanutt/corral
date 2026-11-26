use std::{fs::OpenOptions, io::Write, path::Path};

use anyhow::Result;
use corral_protocol::{
    detail::SessionItem,
    subscription::{
        SessionLiveEvent, SessionLiveGap, SessionLiveKind as Kind, SessionLiveTurnState,
        TranscriptOp as Op,
    },
};
use serde_json::{Value, json};

use super::{
    follower::{Follower, HostOpen},
    reader::LivePage,
};

pub(super) fn append(path: &Path, entry: Value) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{entry}")?;
    Ok(())
}

pub(super) fn page(events: Vec<Kind>) -> LivePage {
    page_after(0, events)
}

/// The host's events after `seq`, to the end of its ring.
pub(super) fn page_after(seq: u64, events: Vec<Kind>) -> LivePage {
    LivePage {
        generation: Some(1),
        next: seq + events.len() as u64,
        host_tail: seq + events.len() as u64,
        more: false,
        gap: None,
        fleet: None,
        state: None,
        events: events
            .into_iter()
            .enumerate()
            .map(|(i, kind)| {
                (
                    SessionLiveEvent {
                        seq: seq + i as u64 + 1,
                        at: "now".into(),
                        generation: 1,
                        kind,
                    },
                    1,
                )
            })
            .collect(),
    }
}

#[test]
fn journal_and_live_words_share_one_tail() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    append(
        &path,
        json!({"type":"message", "id":"u", "parentId":null,"message":{"role":"user","content":"hi"}}),
    )?;
    append(
        &path,
        json!({"type":"message", "id":"u2", "parentId":"u","message":{"role":"user","content":"again"}}),
    )?;
    let mut follower = Follower::default();
    let started = page(vec![
        Kind::LiveTurn {
            turn: 2,
            state: SessionLiveTurnState::Started,
        },
        Kind::LiveText {
            turn: 2,
            message: 1,
            offset: 0,
            text: "hello".into(),
        },
    ]);
    let ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
    assert!(matches!(
        ops.as_slice(),
        [
            Op::Reset { older: None },
            Op::Put { before: None, .. },
            Op::Put {
                before: Some(_),
                ..
            }
        ]
    ));
    let ops = follower.visit("s", &path, None, Some(&started), None);
    assert!(
        matches!(ops.as_slice(), [Op::Put {message, open: true,..}, Op::Append {offset:0,..}] if message.id == "s/open/1.2.1")
    );

    append(
        &path,
        json!({"type":"message", "id":"a", "parentId":"u2","message":{"role":"assistant","content":[{"type":"text","text":"hello"},{"type":"toolCall","id":"c","name":"edit","arguments":{}}]}}),
    )?;
    let ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
    let [
        Op::Put {
            message,
            before: Some(_),
            replaces: Some(replaces),
            open: false,
            cut: false,
        },
    ] = ops.as_slice()
    else {
        panic!("{ops:?}");
    };
    assert_eq!(replaces, "s/open/1.2.1");
    assert_eq!(message.id, "s/entry/a");
    assert!(message.items.iter().any(|item| matches!(
        item,
        SessionItem::ToolCall { call_id, result: None, .. } if call_id == "c"
    )));

    append(
        &path,
        json!({"type":"message", "id":"r", "parentId":"a","message":{"role":"toolResult","toolCallId":"c","toolName":"edit","content":"changed"}}),
    )?;
    let ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
    // The result has its record's id, not the id of the call or the open message.
    assert!(
        matches!(ops.as_slice(), [Op::Put { message, replaces: None, open: false, .. }]
            if message.id == "s/entry/r" && matches!(message.items.as_slice(),
                [SessionItem::ToolCall { call_id, result: Some(_), .. }] if call_id == "c")),
        "{ops:?}"
    );

    append(
        &path,
        json!({"type":"message", "id":"other", "parentId":"u","message":{"role":"user","content":"other branch"}}),
    )?;
    let ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
    assert!(matches!(
        ops.as_slice(),
        [Op::Reset { .. }, Op::Put { .. }, Op::Put { .. }]
    ));
    Ok(())
}

#[test]
fn gap_seeds_the_open_ordinal_and_rebases_offsets() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    append(
        &path,
        json!({"type":"message", "id":"a", "parentId":null,"message":{"role":"assistant","content":"first"}}),
    )?;
    let mut follower = Follower::default();
    let mut gap = page(vec![]);
    gap.gap = Some(SessionLiveGap {
        after: 0,
        resumed: 15,
        missing: 14,
        reconcile: true,
    });
    let seed: HostOpen = serde_json::from_value(
        json!({"turn":2,"message":2,"ended":false,"text_offset":100,"text":"tail","thinking_offset":0,"thinking":""}),
    )?;
    let ops = follower.visit("s", &path, None, Some(&gap), Some(seed));
    assert!(
        matches!(ops.as_slice(), [Op::Reset {..}, Op::Put {open:false,..}, Op::Put {open:true,..}, Op::Append {offset:0,text,..}] if text == "tail")
    );
    let words = page(vec![
        Kind::LiveText {
            turn: 2,
            message: 1,
            offset: 0,
            text: "ignored".into(),
        },
        Kind::LiveText {
            turn: 2,
            message: 2,
            offset: 104,
            text: "more".into(),
        },
    ]);
    let ops = follower.visit("s", &path, None, Some(&words), None);
    assert!(matches!(ops.as_slice(), [Op::Append {offset:4,text,..}] if text == "more"));
    append(
        &path,
        json!({"type":"message", "id":"b", "parentId":"a","message":{"role":"assistant","content":"second"}}),
    )?;
    let ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
    assert!(matches!(ops.as_slice(), [Op::Put {replaces: Some(id),..}] if id == "s/open/1.2.2"));
    Ok(())
}

/// A reader joined a running turn whose start was on the host's second page: the journal's
/// records of the turn had been read with the first, so the words of a recorded message
/// were opened again as a message being written.
#[test]
fn joining_a_turn_does_not_replay_the_words_of_recorded_messages() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    append(
        &path,
        json!({"type":"message", "id":"a", "parentId":null, "timestamp":"2026-11-18T12:00:05Z",
            "message":{"role":"assistant","content":"first"}}),
    )?;
    let mut follower = Follower::default();
    let mut first = page(vec![Kind::LiveQueue {
        steer: 0,
        follow_up: 0,
    }]);
    first.host_tail = 3;
    first.more = true;
    let ops = follower.visit("s", &path, None, Some(&first), None);
    assert!(matches!(
        ops.as_slice(),
        [Op::Reset { .. }, Op::Put { open: false, .. }]
    ));
    let second = page_after(
        1,
        vec![
            Kind::LiveTurn {
                turn: 2,
                state: SessionLiveTurnState::Started,
            },
            Kind::LiveText {
                turn: 2,
                message: 1,
                offset: 0,
                text: "first".into(),
            },
        ],
    );
    let ops = follower.visit("s", &path, None, Some(&second), None);
    assert!(ops.is_empty(), "{ops:?}");
    Ok(())
}

/// A message of seven calls or more was cut as a history page cuts it: the stream carried
/// its last calls only, after messages the reader already held, so the first calls could
/// not be reached.
#[test]
fn a_message_of_many_calls_is_carried_whole_until_it_cannot_be() -> Result<()> {
    let calls = |count: usize, argument: &str| -> Vec<Value> {
        (0..count)
            .map(|n| json!({"type":"toolCall","id":format!("c{n}"),"name":"read","arguments":{"path":argument}}))
            .collect()
    };
    let held = |op: &Op| match op {
        Op::Put { message, cut, .. } => (
            message.id.clone(),
            message
                .items
                .iter()
                .filter(|item| matches!(item, SessionItem::ToolCall { .. }))
                .count(),
            *cut,
        ),
        _ => panic!("{op:?}"),
    };
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    append(
        &path,
        json!({"type":"message", "id":"u", "parentId":null,"message":{"role":"user","content":"hi"}}),
    )?;
    append(
        &path,
        json!({"type":"message", "id":"a", "parentId":"u","message":{"role":"assistant","content":calls(8, "a")}}),
    )?;
    let mut follower = Follower::default();
    // A reader that joins is given the newest record whole too.
    let ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
    let [Op::Reset { older: Some(_) }, joined] = ops.as_slice() else {
        panic!("{ops:?}");
    };
    assert_eq!(held(joined), ("s/entry/a".into(), 8, false));

    append(
        &path,
        json!({"type":"message", "id":"b", "parentId":"a","message":{"role":"assistant","content":calls(8, "b")}}),
    )?;
    let ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
    let [followed] = ops.as_slice() else {
        panic!("{ops:?}");
    };
    assert_eq!(held(followed), ("s/entry/b".into(), 8, false));

    // More calls than one operation holds at any preview size: the reader starts again at
    // the message's last calls and pages back to the rest. The turn it is part of goes on.
    let started = page(vec![Kind::LiveTurn {
        turn: 2,
        state: SessionLiveTurnState::Started,
    }]);
    assert!(
        follower
            .visit("s", &path, None, Some(&started), None)
            .is_empty()
    );
    append(
        &path,
        json!({"type":"message", "id":"c", "parentId":"b","message":{"role":"assistant","content":calls(120, &"x".repeat(3000))}}),
    )?;
    let ops = follower.visit("s", &path, None, Some(&page_after(1, vec![])), None);
    let [last] = ops.as_slice() else {
        panic!("{ops:?}");
    };
    let (id, count, cut) = held(last);
    assert_eq!((id.as_str(), cut), ("s/entry/c", true));
    assert!((1..120).contains(&count));
    let mut buffer = super::transcript_buffer::TranscriptBuffer::default();
    buffer.publish(vec![joined.clone(), followed.clone(), last.clone()]);
    let tail: Vec<Op> = buffer.snapshot().into_iter().map(|(op, _)| op).collect();
    assert!(matches!(
        tail.as_slice(),
        [Op::Reset { older: Some(_) }, Op::Put { cut: true, .. }]
    ));
    let next = page_after(
        1,
        vec![Kind::LiveText {
            turn: 2,
            message: 2,
            offset: 0,
            text: "next".into(),
        }],
    );
    let ops = follower.visit("s", &path, None, Some(&next), None);
    assert!(
        matches!(ops.as_slice(), [Op::Put { message, open: true, .. }, Op::Append { .. }] if message.id == "s/open/1.2.2"),
        "{ops:?}"
    );
    Ok(())
}

/// The host answers before the journal is read. A message's record that reached the
/// journal in between was put and opened again as being written; one that had ended but
/// was not yet in the journal was counted before it came, and the next message's words
/// were taken for recorded.
#[test]
fn a_join_tells_whether_the_latest_message_is_in_the_journal_it_read() -> Result<()> {
    let seed = |ended: bool| -> Result<HostOpen> {
        Ok(serde_json::from_value(
            json!({"turn":2,"message":1,"ended":ended,
            "message_at":"2026-11-18T12:00:01Z",
            "text_offset":0,"text":if ended {""} else {"fir"},"thinking_offset":0,"thinking":""}),
        )?)
    };
    let record = |id: &str, parent: Option<&str>| {
        json!({"type":"message", "id":id, "parentId":parent, "timestamp":"2026-11-18T12:00:05Z",
            "message":{"role":"assistant","content":"first"}})
    };
    let next = || {
        page(vec![Kind::LiveText {
            turn: 2,
            message: 2,
            offset: 0,
            text: "second".into(),
        }])
    };
    let opened = |ops: &[Op]| matches!(ops, [Op::Put { message, open: true, .. }, Op::Append { .. }] if message.id == "s/open/1.2.2");
    let dir = tempfile::tempdir()?;

    // Still being written when the host answered, recorded by the time the journal is read.
    let path = dir.path().join("recorded.jsonl");
    append(&path, record("a", None))?;
    let mut follower = Follower::default();
    let ops = follower.visit("s", &path, None, Some(&page(vec![])), Some(seed(false)?));
    assert!(
        matches!(
            ops.as_slice(),
            [Op::Reset { .. }, Op::Put { open: false, .. }]
        ),
        "{ops:?}"
    );
    let ops = follower.visit("s", &path, None, Some(&next()), None);
    assert!(opened(&ops), "{ops:?}");

    // Recorded, and the message after it too.
    let path = dir.path().join("two.jsonl");
    append(&path, record("a", None))?;
    append(&path, record("b", Some("a")))?;
    let mut follower = Follower::default();
    follower.visit("s", &path, None, Some(&page(vec![])), Some(seed(false)?));
    let ops = follower.visit("s", &path, None, Some(&next()), None);
    assert!(ops.is_empty(), "{ops:?}");

    // Not started when the host answered, recorded by the time the journal is read.
    let path = dir.path().join("early.jsonl");
    append(&path, record("a", None))?;
    let mut follower = Follower::default();
    let waiting = serde_json::from_value(json!({"turn":2,"message":0,"ended":true,
        "message_at":"2026-11-18T12:00:01Z",
        "text_offset":0,"text":"","thinking_offset":0,"thinking":""}))?;
    follower.visit("s", &path, None, Some(&page(vec![])), Some(waiting));
    let first = page(vec![Kind::LiveText {
        turn: 2,
        message: 1,
        offset: 0,
        text: "first".into(),
    }]);
    let ops = follower.visit("s", &path, None, Some(&first), None);
    assert!(ops.is_empty(), "{ops:?}");
    let ops = follower.visit("s", &path, None, Some(&next()), None);
    assert!(opened(&ops), "{ops:?}");

    // Ended when the host answered, not yet in the journal.
    let path = dir.path().join("pending.jsonl");
    append(
        &path,
        json!({"type":"message", "id":"u", "parentId":null, "timestamp":"2026-11-18T12:00:00Z",
            "message":{"role":"user","content":"hi"}}),
    )?;
    let mut follower = Follower::default();
    follower.visit("s", &path, None, Some(&page(vec![])), Some(seed(true)?));
    append(&path, record("a", Some("u")))?;
    let ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
    assert!(
        matches!(ops.as_slice(), [Op::Put { open: false, .. }]),
        "{ops:?}"
    );
    let ops = follower.visit("s", &path, None, Some(&next()), None);
    assert!(opened(&ops), "{ops:?}");
    Ok(())
}

/// A journal read that failed made the next one a join without the host's word on the turn:
/// the reply being written stopped until the next turn.
#[test]
fn a_failed_journal_read_does_not_end_the_reply_being_written() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    append(
        &path,
        json!({"type":"message", "id":"u", "parentId":null,"message":{"role":"user","content":"hi"}}),
    )?;
    let mut follower = Follower::default();
    follower.visit("s", &path, None, Some(&page(vec![])), None);
    let writing = page(vec![
        Kind::LiveTurn {
            turn: 2,
            state: SessionLiveTurnState::Started,
        },
        Kind::LiveText {
            turn: 2,
            message: 1,
            offset: 0,
            text: "he".into(),
        },
    ]);
    follower.visit("s", &path, None, Some(&writing), None);
    let words = |seq: u64, offset: u64, text: &str| {
        page_after(
            seq,
            vec![Kind::LiveText {
                turn: 2,
                message: 1,
                offset,
                text: text.into(),
            }],
        )
    };
    // The reader does not ask the host for this page again: its words are followed.
    let missing = dir.path().join("missing.jsonl");
    let ops = follower.visit("s", &missing, None, Some(&words(2, 2, "ll")), None);
    assert!(
        matches!(ops.as_slice(), [Op::Append { offset: 2, text, .. }] if text == "ll"),
        "{ops:?}"
    );
    let ops = follower.visit("s", &path, None, Some(&words(3, 4, "o")), None);
    // The tail again, then the message from the words the host writes next.
    assert!(
        matches!(ops.as_slice(), [Op::Reset { .. }, Op::Put { open: false, .. }, Op::Put { message, open: true, .. }, Op::Append { offset: 0, text, .. }] if message.id == "s/open/1.2.1" && text == "o"),
        "{ops:?}"
    );
    Ok(())
}

/// The tail read again after a failed read is one page. A reply recorded while the journal
/// could not be read, with more than a page written after it, was not among its messages:
/// it was put again as being written, and the next record took its place.
#[test]
fn a_reply_recorded_before_the_tail_read_again_is_not_put_as_being_written() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let entry = |id: &str, parent: Option<&str>, role: &str, second: u8| {
        json!({"type":"message", "id":id, "parentId":parent,
            "timestamp":format!("2026-11-18T12:00:0{second}Z"),
            "message":{"role":role,"content":"words"}})
    };
    let text = |message: u64| Kind::LiveText {
        turn: 2,
        message,
        offset: 0,
        text: "words".into(),
    };
    let started = |words: Vec<Kind>| {
        let mut events = vec![Kind::LiveTurn {
            turn: 2,
            state: SessionLiveTurnState::Started,
        }];
        events.extend(words);
        let mut page = page(events);
        page.events[0].0.at = "2026-11-18T12:00:01Z".into();
        page
    };
    // The turn's start reaches the follower before the read fails, or only after it.
    for late in [false, true] {
        let path = dir.path().join(format!("{late}.jsonl"));
        append(&path, entry("u", None, "user", 0))?;
        let mut follower = Follower::default();
        follower.visit("s", &path, None, Some(&page(vec![])), None);
        let mut read = 0;
        if !late {
            follower.visit("s", &path, None, Some(&started(vec![text(1)])), None);
            read = 2;
        }
        let missing = dir.path().join("missing.jsonl");
        follower.visit("s", &missing, None, Some(&page_after(read, vec![])), None);
        append(&path, entry("a", Some("u"), "assistant", 5))?;
        let mut parent = "a".to_owned();
        for n in 0..40 {
            let id = format!("n{n}");
            append(&path, entry(&id, Some(&parent), "user", 6))?;
            parent = id;
        }
        let recovery = if late {
            started(vec![text(1), text(2)])
        } else {
            page_after(read, vec![text(2)])
        };
        let ops = follower.visit("s", &path, None, Some(&recovery), None);
        // Which of the turn's messages are recorded is not known: its words are left out.
        assert!(
            matches!(ops.first(), Some(Op::Reset { older: Some(_) })),
            "{ops:?}"
        );
        assert!(
            ops.iter()
                .all(|op| matches!(op, Op::Reset { .. } | Op::Put { open: false, .. })),
            "{ops:?}"
        );
    }
    Ok(())
}

/// A large enough journal suffix pushes an older call out of a new subscriber's tail.
/// Its late result must still be delivered at the end, both live and at join.
#[test]
fn a_result_for_a_call_before_the_tail_goes_as_its_own_message() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    append(
        &path,
        json!({"type":"message", "id":"u", "parentId":null,"message":{"role":"user","content":"hi"}}),
    )?;
    append(
        &path,
        json!({"type":"message", "id":"a", "parentId":"u","message":{"role":"assistant","content":[{"type":"toolCall","id":"c","name":"edit","arguments":{}}]}}),
    )?;
    let mut follower = Follower::default();
    let mut buffer = super::transcript_buffer::TranscriptBuffer::default();
    let ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
    buffer.publish(ops);
    // Enough after the call for the tail subscribers are given to begin past it.
    let long: Vec<Value> = (0..12)
        .map(|_| json!({"type":"text","text":"x".repeat(4000)}))
        .collect();
    let mut parent = "a".to_owned();
    for n in 0..6 {
        let id = format!("f{n}");
        append(
            &path,
            json!({"type":"message", "id":id, "parentId":parent,"message":{"role":"assistant","content":long}}),
        )?;
        parent = id;
    }
    let ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
    buffer.publish(ops);
    let ids = |buffer: &super::transcript_buffer::TranscriptBuffer| -> Vec<String> {
        let rows = buffer.snapshot().into_iter();
        rows.filter_map(|(op, _)| match op {
            Op::Put { message, .. } => Some(message.id),
            _ => None,
        })
        .collect()
    };
    let held = ids(&buffer);
    assert!(!held.iter().any(|id| id == "s/entry/a"), "{held:?}");
    assert_eq!(held.last().map(String::as_str), Some("s/entry/f5"));
    append(
        &path,
        json!({"type":"message", "id":"r", "parentId":parent,"message":{"role":"toolResult","toolCallId":"c","toolName":"edit","content":"changed"}}),
    )?;
    let ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
    assert!(
        matches!(ops.as_slice(), [Op::Put { message, replaces: None, .. }] if message.id == "s/entry/r"),
        "{ops:?}"
    );
    buffer.publish(ops);
    let Some((Op::Put { message, .. }, _)) = buffer.operations.back() else {
        panic!("a put");
    };
    assert_eq!(message.id, "s/entry/r");
    assert!(
        matches!(
            message.items.as_slice(),
            [SessionItem::ToolCall {
                result: Some(_),
                ..
            }]
        ),
        "{message:?}"
    );
    assert_eq!(ids(&buffer).last().map(String::as_str), Some("s/entry/r"));
    Ok(())
}

/// A tail that begins inside a long record is read forward from that record's start, a
/// bounded step at a time. One step did not reach the journal's end, and what it had read
/// was taken for the tail: the records after it came later, behind the message the host
/// was writing.
#[test]
fn a_tail_read_forward_from_a_long_record_reaches_the_journals_end() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    // More raw bytes than one step reads, in more items than one page holds.
    let long: Vec<Value> = (0..60)
        .map(|_| json!({"type":"text","text":"x".repeat(10_000)}))
        .collect();
    append(
        &path,
        json!({"type":"message", "id":"a", "parentId":null,"message":{"role":"assistant","content":long}}),
    )?;
    append(
        &path,
        json!({"type":"message", "id":"u", "parentId":"a","message":{"role":"user","content":"after"}}),
    )?;
    let ops = Follower::default().visit("s", &path, None, Some(&page(vec![])), None);
    assert!(
        matches!(ops.as_slice(), [Op::Reset { .. }, Op::Put { message: first, .. }, Op::Put { message: last, .. }]
            if first.id == "s/entry/a" && last.id == "s/entry/u"),
        "{:?}",
        ops.iter()
            .map(|op| match op {
                Op::Put { message, cut, .. } => format!("{} cut={cut}", message.id),
                other => format!("{other:?}"),
            })
            .collect::<Vec<_>>()
    );
    Ok(())
}

/// Records are told apart by where they are in the journal, not by their stamps alone: a
/// reply stamped the same instant as the one before it was taken for that one, and its
/// words stayed beside its record.
#[test]
fn replies_stamped_alike_each_take_their_own_message() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    let entry = |id: &str, parent: &str, role: &str, second: u8| {
        json!({"type":"message", "id":id, "parentId":parent,
            "timestamp":format!("2026-11-18T12:00:0{second}Z"),
            "message":{"role":role,"content":"words"}})
    };
    append(
        &path,
        json!({"type":"message", "id":"u", "parentId":null,
            "timestamp":"2026-11-18T12:00:00Z","message":{"role":"user","content":"hi"}}),
    )?;
    let mut follower = Follower::default();
    follower.visit("s", &path, None, Some(&page(vec![])), None);
    let text = |message: u64| Kind::LiveText {
        turn: 2,
        message,
        offset: 0,
        text: "words".into(),
    };
    let mut started = page(vec![
        Kind::LiveTurn {
            turn: 2,
            state: SessionLiveTurnState::Started,
        },
        text(1),
    ]);
    started.events[0].0.at = "2026-11-18T12:00:01Z".into();
    follower.visit("s", &path, None, Some(&started), None);
    let replaced = |ops: &[Op], open: &str| matches!(ops.first(), Some(Op::Put { replaces: Some(id), .. }) if id == open);
    append(&path, entry("a", "u", "assistant", 5))?;
    let ops = follower.visit("s", &path, None, Some(&page_after(2, vec![text(2)])), None);
    assert!(replaced(&ops, "s/open/1.2.1"), "{ops:?}");
    append(&path, entry("b", "a", "assistant", 5))?;
    let ops = follower.visit("s", &path, None, Some(&page_after(3, vec![text(3)])), None);
    assert!(replaced(&ops, "s/open/1.2.2"), "{ops:?}");

    // Counted again from the tail after a read that failed: the third reply, stamped like
    // the two before it and recorded meanwhile, is not put again as being written.
    let missing = dir.path().join("missing.jsonl");
    follower.visit("s", &missing, None, Some(&page_after(4, vec![])), None);
    append(&path, entry("c", "b", "assistant", 5))?;
    let ops = follower.visit("s", &path, None, Some(&page_after(4, vec![])), None);
    assert!(
        matches!(ops.first(), Some(Op::Reset { .. }))
            && ops[1..]
                .iter()
                .all(|op| matches!(op, Op::Put { open: false, .. })),
        "{ops:?}"
    );
    Ok(())
}

/// The tail reads a bounded number of records at a time. The last record of a turn, left
/// for the visit after the page that starts the next turn, was counted as that turn's first
/// and took the place of the message it was writing.
#[test]
fn the_journal_is_read_to_its_end_before_the_hosts_page_is_followed() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    let entry = |id: &str, parent: Option<&str>, role: &str, second: u8| {
        json!({"type":"message", "id":id, "parentId":parent,
            "timestamp":format!("2026-11-18T12:00:0{second}Z"),
            "message":{"role":role,"content":"words"}})
    };
    append(&path, entry("u", None, "user", 0))?;
    let mut follower = Follower::default();
    follower.visit("s", &path, None, Some(&page(vec![])), None);
    // More than one read holds, then the earlier turn's last reply.
    let mut parent = "u".to_owned();
    for n in 0..33 {
        let id = format!("n{n}");
        append(&path, entry(&id, Some(&parent), "user", 1))?;
        parent = id;
    }
    append(&path, entry("last", Some(&parent), "assistant", 2))?;
    let mut started = page(vec![
        Kind::LiveTurn {
            turn: 2,
            state: SessionLiveTurnState::Started,
        },
        Kind::LiveText {
            turn: 2,
            message: 1,
            offset: 0,
            text: "words".into(),
        },
    ]);
    started.events[0].0.at = "2026-11-18T12:00:03Z".into();
    let ops = follower.visit("s", &path, None, Some(&started), None);
    // Every record first, then the message the new turn is writing.
    let [records @ .., record, open, Op::Append { .. }] = ops.as_slice() else {
        panic!("{ops:?}");
    };
    assert_eq!(records.len(), 33, "{ops:?}");
    assert!(
        matches!(record, Op::Put { message, replaces: None, open: false, .. } if message.author == "assistant"),
        "{ops:?}"
    );
    assert!(
        matches!(open, Op::Put { message, open: true, .. } if message.id == "s/open/1.2.1"),
        "{ops:?}"
    );
    let ops = follower.visit("s", &path, None, Some(&page_after(2, vec![])), None);
    assert!(ops.is_empty(), "{ops:?}");
    Ok(())
}

/// The journal is read to its end on every visit and the host a page at a time: a turn's
/// record read before the page that starts the turn was not counted, and the words of that
/// message were opened after it.
#[test]
fn a_record_read_before_its_turn_starts_is_counted_when_it_does() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    append(
        &path,
        json!({"type":"message", "id":"u", "parentId":null, "timestamp":"2026-11-18T12:00:00Z",
            "message":{"role":"user","content":"hi"}}),
    )?;
    let mut follower = Follower::default();
    follower.visit("s", &path, None, Some(&page(vec![])), None);
    append(
        &path,
        json!({"type":"message", "id":"a", "parentId":"u", "timestamp":"2026-11-18T12:00:05Z",
            "message":{"role":"assistant","content":"first"}}),
    )?;
    follower.visit("s", &path, None, Some(&page(vec![])), None);
    let text = |message: u64| Kind::LiveText {
        turn: 2,
        message,
        offset: 0,
        text: "words".into(),
    };
    let mut started = page(vec![
        Kind::LiveTurn {
            turn: 2,
            state: SessionLiveTurnState::Started,
        },
        text(1),
        text(2),
    ]);
    started.events[0].0.at = "2026-11-18T12:00:01Z".into();
    let ops = follower.visit("s", &path, None, Some(&started), None);
    assert!(
        matches!(ops.as_slice(), [Op::Put { message, open: true, .. }, Op::Append { .. }] if message.id == "s/open/1.2.2"),
        "{ops:?}"
    );
    Ok(())
}

/// A subscriber that fell behind a very long reply was given its first words and then
/// appends from further on, which it could not place.
#[test]
fn a_long_reply_stops_at_the_bound_its_tail_holds() -> Result<()> {
    use super::{follower::OPEN_PART_BYTES, transcript_buffer::TranscriptBuffer};
    let written = |ops: &[Op]| {
        ops.iter()
            .map(|op| match op {
                Op::Append { offset, text, .. } => {
                    assert!(offset + text.len() as u64 <= OPEN_PART_BYTES);
                    text.len() as u64
                }
                _ => 0,
            })
            .sum::<u64>()
    };
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    append(
        &path,
        json!({"type":"message", "id":"u", "parentId":null,"message":{"role":"user","content":"hi"}}),
    )?;
    let mut follower = Follower::default();
    let mut ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
    let long = page(vec![
        Kind::LiveTurn {
            turn: 2,
            state: SessionLiveTurnState::Started,
        },
        Kind::LiveText {
            turn: 2,
            message: 1,
            offset: 0,
            text: "a".repeat(OPEN_PART_BYTES as usize - 1),
        },
        // The two-byte character would end past the bound, so none of this is carried.
        Kind::LiveText {
            turn: 2,
            message: 1,
            offset: OPEN_PART_BYTES - 1,
            text: "éz".into(),
        },
        Kind::LiveText {
            turn: 2,
            message: 1,
            offset: OPEN_PART_BYTES + 2,
            text: "past".into(),
        },
    ]);
    ops.extend(follower.visit("s", &path, None, Some(&long), None));
    assert_eq!(written(&ops), OPEN_PART_BYTES - 1);
    let mut buffer = TranscriptBuffer::default();
    buffer.publish(ops);
    let snapshot: Vec<Op> = buffer.snapshot().into_iter().map(|(op, _)| op).collect();
    assert_eq!(written(&snapshot), OPEN_PART_BYTES - 1);
    Ok(())
}
