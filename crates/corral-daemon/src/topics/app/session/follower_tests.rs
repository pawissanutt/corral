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
    LivePage {
        generation: Some(1),
        next: events.len() as u64,
        host_tail: events.len() as u64,
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
                        seq: i as u64 + 1,
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
    let ops = follower.visit("s", &path, None, Some(&started), None);
    assert!(
        matches!(ops.as_slice(), [Op::Reset {older: None}, Op::Put {before: None, ..}, Op::Put {before: Some(_), ..}, Op::Put {message, open: true,..}, Op::Append {offset:0,..}] if message.id == "s/open/1.2.1")
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
