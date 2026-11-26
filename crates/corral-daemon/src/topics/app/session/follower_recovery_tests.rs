use anyhow::Result;
use corral_protocol::subscription::{
    SessionLiveKind as Kind, SessionLiveTurnState, TranscriptOp as Op,
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

/// A result folded into a call's message was in that message alone. When the tail given to
/// a subscriber that joins came to begin after the call but before the result's record,
/// the result was in neither the tail nor any page before it.
#[test]
fn a_folded_result_stays_in_the_tail_when_its_call_leaves_it() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    let said = |id: &str, parent: &str| json!({"type":"message", "id":id, "parentId":parent,"message":{"role":"user","content":"words"}});
    append(
        &path,
        json!({"type":"message", "id":"a", "parentId":null,"message":{"role":"assistant","content":[{"type":"toolCall","id":"c","name":"edit","arguments":{}}]}}),
    )?;
    let mut follower = Follower::default();
    let mut buffer = TranscriptBuffer::default();
    let read = |follower: &mut Follower, buffer: &mut TranscriptBuffer| {
        let ops = follower.visit("s", &path, None, Some(&page(vec![])), None);
        buffer.publish(ops, follower.take_alone());
    };
    read(&mut follower, &mut buffer);
    // As many messages as the tail holds beside the call.
    let mut parent = "a".to_owned();
    for n in 0..31 {
        let id = format!("m{n}");
        append(&path, said(&id, &parent))?;
        parent = id;
    }
    read(&mut follower, &mut buffer);
    append(
        &path,
        json!({"type":"message", "id":"r", "parentId":parent,"message":{"role":"toolResult","toolCallId":"c","toolName":"edit","content":"changed"}}),
    )?;
    read(&mut follower, &mut buffer);
    assert_eq!(ids(&buffer).first().map(String::as_str), Some("s/entry/a"));
    assert!(!ids(&buffer).contains(&"s/entry/r".to_owned()));
    // One more, and the tail begins after the call.
    append(&path, said("last", "r"))?;
    read(&mut follower, &mut buffer);
    let held = ids(&buffer);
    assert_eq!(
        held[held.len() - 3..],
        ["s/entry/m30", "s/entry/r", "s/entry/last"],
        "{held:?}"
    );
    assert!(!held.contains(&"s/entry/a".to_owned()));
    Ok(())
}
