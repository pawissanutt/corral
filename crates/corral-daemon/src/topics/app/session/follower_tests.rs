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

fn append(path: &Path, entry: Value) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{entry}")?;
    Ok(())
}

fn page(events: Vec<Kind>) -> LivePage {
    page_after(0, events)
}

/// The host's events after `seq`, to the end of its ring.
fn page_after(seq: u64, events: Vec<Kind>) -> LivePage {
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
    let id = message.id.clone();

    append(
        &path,
        json!({"type":"message", "id":"r", "parentId":"a","message":{"role":"toolResult","toolCallId":"c","toolName":"edit","content":"changed"}}),
    )?;
    let ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
    assert!(
        matches!(ops.as_slice(), [Op::Put {message,..}] if message.id == id && message.items.iter().any(|item| matches!(item, SessionItem::ToolCall {result: Some(_),..})))
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
    let path = dir.path().join("journal.jsonl");
    let entry = |id: &str, parent: Option<&str>, role: &str, second: u8| {
        json!({"type":"message", "id":id, "parentId":parent,
            "timestamp":format!("2026-11-18T12:00:0{second}Z"),
            "message":{"role":role,"content":"words"}})
    };
    append(&path, entry("u", None, "user", 0))?;
    let mut follower = Follower::default();
    follower.visit("s", &path, None, Some(&page(vec![])), None);
    let text = |message: u64| Kind::LiveText {
        turn: 2,
        message,
        offset: 0,
        text: "words".into(),
    };
    let mut writing = page(vec![
        Kind::LiveTurn {
            turn: 2,
            state: SessionLiveTurnState::Started,
        },
        text(1),
    ]);
    writing.events[0].0.at = "2026-11-18T12:00:01Z".into();
    follower.visit("s", &path, None, Some(&writing), None);
    let missing = dir.path().join("missing.jsonl");
    follower.visit("s", &missing, None, Some(&page_after(2, vec![])), None);
    append(&path, entry("a", Some("u"), "assistant", 5))?;
    let mut parent = "a".to_owned();
    for n in 0..40 {
        let id = format!("n{n}");
        append(&path, entry(&id, Some(&parent), "user", 6))?;
        parent = id;
    }
    let ops = follower.visit("s", &path, None, Some(&page_after(2, vec![text(2)])), None);
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
