use anyhow::Result;
use corral_protocol::{
    detail::SessionItem,
    subscription::{SessionLiveKind as Kind, SessionLiveTurnState, TranscriptOp as Op},
};
use serde_json::json;

use super::{
    follower::Follower,
    follower_tests::{append, page, page_after},
    transcript_buffer::TranscriptBuffer,
};

fn ids(buffer: &TranscriptBuffer) -> Vec<String> {
    let rows = buffer.snapshot().into_iter();
    rows.filter_map(|(op, _)| match op {
        Op::Put { message, .. } => Some(message.id),
        _ => None,
    })
    .collect()
}

/// A tail read again that begins at a record stamped like the last one paired was taken to
/// hold every record after it. It did not hold that record, nothing after it was counted,
/// and a reply the tail already had was put again as being written, with no record left to
/// take its place.
#[test]
fn a_tail_without_the_record_last_paired_does_not_place_the_turn() -> Result<()> {
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
    let mut started = page(vec![
        Kind::LiveTurn {
            turn: 2,
            state: SessionLiveTurnState::Started,
        },
        text(1),
    ]);
    started.events[0].0.at = "2026-11-18T12:00:01Z".into();
    follower.visit("s", &path, None, Some(&started), None);
    append(&path, entry("a", Some("u"), "assistant", 5))?;
    follower.visit("s", &path, None, Some(&page_after(2, vec![text(2)])), None);
    let missing = dir.path().join("missing.jsonl");
    follower.visit("s", &missing, None, Some(&page_after(3, vec![])), None);
    // More than a tail holds after the reply paired, all of it stamped like that reply,
    // and then the second reply.
    let mut parent = "a".to_owned();
    for n in 0..33 {
        let id = format!("n{n}");
        append(&path, entry(&id, Some(&parent), "user", 5))?;
        parent = id;
    }
    append(&path, entry("b", Some(&parent), "assistant", 5))?;
    let ops = follower.visit("s", &path, None, Some(&page_after(3, vec![])), None);
    assert!(
        matches!(ops.first(), Some(Op::Reset { older: Some(_) })),
        "{ops:?}"
    );
    assert!(
        ops[1..]
            .iter()
            .all(|op| matches!(op, Op::Put { open: false, .. })),
        "{ops:?}"
    );
    Ok(())
}

#[test]
fn a_dense_preview_and_its_separate_result_keep_the_same_call_id() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    append(
        &path,
        json!({"type":"message", "id":"u", "parentId":null, "message":{"role":"user", "content":"words"}}),
    )?;
    let mut follower = Follower::default();
    follower.visit("s", &path, None, Some(&page(vec![])), None);
    // Unequal preview budgets must not change the pairing key. The dense call record
    // clips its arguments much harder than its standalone result does.
    let call_id = |n| format!("call_{}-{n}", "a".repeat(90));
    let dense: Vec<_> = (0..40)
        .map(|n| json!({"type":"toolCall","id":call_id(n),"name":"read","arguments":{"path":"x".repeat(3000)}}))
        .collect();
    append(
        &path,
        json!({"type":"message", "id":"dense", "parentId":"u","message":{"role":"assistant","content":dense}}),
    )?;
    append(
        &path,
        json!({"type":"message", "id":"dense-result", "parentId":"dense","message":{"role":"toolResult","toolCallId":call_id(1),"toolName":"read","content":"answer"}}),
    )?;
    let ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
    let [
        Op::Put {
            message: dense,
            cut: false,
            ..
        },
        Op::Put {
            message: result, ..
        },
    ] = ops.as_slice()
    else {
        panic!("expected separate puts for the whole dense record and its result");
    };
    let key = call_id(1);
    assert!(
        dense
            .items
            .iter()
            .any(|item| matches!(item, SessionItem::Continuation { .. }))
    );
    for message in [dense, result] {
        assert!(
            message.items.iter().any(
                |item| matches!(item, SessionItem::ToolCall { call_id, .. } if call_id == &key)
            ),
            "missing complete call id in {}",
            message.id
        );
    }

    Ok(())
}

/// Adjacent results must keep their journal order even when their calls leave a full
/// subscriber tail on separate visits. A fresh reader must see those same records.
#[test]
fn results_remain_ordered_as_their_calls_leave_the_tail_one_at_a_time() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    let said = |id: &str, parent: &str| json!({"type":"message", "id":id, "parentId":parent,"message":{"role":"user","content":"words"}});
    let call = |id: &str, parent: Option<&str>, tool: &str| json!({"type":"message", "id":id, "parentId":parent,"message":{"role":"assistant","content":[{"type":"toolCall","id":tool,"name":"edit","arguments":{}}]}});
    let result = |id: &str, parent: &str, tool: &str| json!({"type":"message", "id":id, "parentId":parent,"message":{"role":"toolResult","toolCallId":tool,"toolName":"edit","content":"changed"}});
    append(&path, call("a", None, "c1"))?;
    append(&path, call("b", Some("a"), "c2"))?;
    let mut follower = Follower::default();
    let mut buffer = TranscriptBuffer::default();
    let read = |follower: &mut Follower, buffer: &mut TranscriptBuffer| {
        buffer.publish(follower.visit("s", &path, None, Some(&page(vec![])), None));
    };
    read(&mut follower, &mut buffer);
    let mut parent = "b".to_owned();
    let mut expected = vec!["s/entry/a".to_owned(), "s/entry/b".to_owned()];
    for n in 0..28 {
        let id = format!("m{n}");
        append(&path, said(&id, &parent))?;
        expected.push(format!("s/entry/{id}"));
        parent = id;
    }
    append(&path, result("r1", &parent, "c1"))?;
    append(&path, result("r2", "r1", "c2"))?;
    read(&mut follower, &mut buffer);
    expected.extend(["s/entry/r1".to_owned(), "s/entry/r2".to_owned()]);
    assert_eq!(ids(&buffer), expected);

    // At capacity, each extra record shifts the tail and its paging cursor by one.
    for (id, parent) in [("last1", "r2"), ("last2", "last1")] {
        append(&path, said(id, parent))?;
        read(&mut follower, &mut buffer);
        expected.remove(0);
        expected.push(format!("s/entry/{id}"));
        let mut fresh = TranscriptBuffer::default();
        read(&mut Follower::default(), &mut fresh);
        assert_eq!(ids(&buffer), expected);
        assert_eq!(buffer.snapshot(), fresh.snapshot());
    }
    Ok(())
}
