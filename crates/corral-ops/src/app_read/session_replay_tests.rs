use super::super::transcript;
use super::*;
use corral_protocol::detail::{DetailBody, DetailReply, SessionPage};

fn fixture(entries: &[Value]) -> Result<(tempfile::TempDir, std::path::PathBuf)> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("session.jsonl");
    let bytes: String = entries.iter().map(|entry| format!("{entry}\n")).collect();
    std::fs::write(&path, bytes)?;
    Ok((directory, path))
}

#[test]
fn tool_result_in_later_entry_is_not_paired_with_its_call() -> Result<()> {
    let (_directory, path) = fixture(&[
        json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"c1","name":"edit","arguments":{"path":"x","oldText":"a","newText":"b"}}]}}),
        json!({"type":"message","message":{"role":"toolResult","toolCallId":"c1","toolName":"edit","content":[{"type":"text","text":"changed"}],"details":{"diff":"-a\n+b"},"durationMs":7,"isError":false}}),
    ])?;
    let (messages, older) = transcript::messages(&path, "s1", None)?;
    assert!(older.is_none());
    let items: Vec<_> = messages.iter().flat_map(|message| &message.items).collect();
    let [
        SessionItem::ToolCall {
            paired,
            state,
            output,
            elapsed_ms,
            diff,
            arguments,
            ..
        },
    ] = items.as_slice()
    else {
        panic!("expected one paired row");
    };
    assert!(*paired);
    assert_eq!(*state, SessionToolState::Done);
    assert_eq!(output.as_deref(), Some("changed"));
    assert_eq!(*elapsed_ms, Some(7));
    assert_eq!(diff.as_deref(), Some("-a\n+b"));
    assert_eq!(serde_json::from_str::<Value>(arguments)?["path"], "x");
    Ok(())
}

#[test]
fn thinking_or_compaction_entry_is_dropped() -> Result<()> {
    let (_directory, path) = fixture(&[
        json!({"type":"message","message":{"role":"assistant","content":[{"type":"thinking","thinking":"reason"}]}}),
        json!({"type":"compaction","summary":"summary","tokensBefore":42}),
    ])?;
    let (messages, _) = transcript::messages(&path, "s1", None)?;
    assert!(matches!(&messages[0].items[0], SessionItem::Thinking { text } if text == "reason"));
    assert!(
        matches!(&messages[1].items[0], SessionItem::Compaction { summary, tokens_before: 42 } if summary == "summary")
    );
    Ok(())
}

#[test]
fn huge_tool_output_exceeds_reply_bound_instead_of_yielding_continuation() -> Result<()> {
    // Escapes make this one record larger than the reverse-read window as well as CONTROL.
    let output = "\0".repeat(24 * 1024);
    let entry = json!({"type":"message","message":{"role":"toolResult","toolCallId":"c1","toolName":"bash","content":[{"type":"text","text":output}],"isError":false}});
    let expected = message(&entry).unwrap().items.remove(0);
    let (_directory, path) = fixture(&[entry])?;
    let (messages, older) = transcript::messages(&path, "s1", None)?;
    assert!(older.is_none());
    let Some(SessionItem::Continuation { token, bytes }) = messages[0].items.last() else {
        panic!("missing continuation");
    };
    assert!(decode("another-session", token).is_err());
    assert!(decode("s1", &format!("{token}0")).is_err());
    let page = SessionPage {
        id: "s1".into(),
        role: "worker".into(),
        task: None,
        run: None,
        machine: None,
        messages: messages.clone(),
        older: None,
        replyable: false,
        reply_refusal: None,
    };
    let reply = DetailReply {
        etag: "a".repeat(64),
        as_of: "2026-01-01T00:00:00Z".into(),
        seq: 0,
        next: None,
        body: Some(DetailBody::Session(page)),
    };
    assert!(
        serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"r","result":reply}))?.len()
            < corral_protocol::app::APP_CONTROL_MAX
    );
    let mut next = Some(token.clone());
    let mut restored = String::new();
    while let Some(raw) = next {
        let token = decode("s1", &raw)?;
        let fragment = fragment("s1", token, expected.clone())?;
        assert_eq!(fragment.offset as usize, restored.len());
        assert_eq!(fragment.bytes, *bytes);
        assert!(
            serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"r","result":{"body":fragment}}))?
                .len()
                < corral_protocol::app::APP_CONTROL_MAX
        );
        restored.push_str(&fragment.json);
        next = fragment.next;
    }
    assert_eq!(serde_json::from_str::<SessionItem>(&restored)?, expected);
    Ok(())
}
