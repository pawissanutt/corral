use std::fs;
use std::io::Write;
use std::path::PathBuf;

use corral_protocol::detail::SessionToolState;
use serde_json::json;

use super::*;

const ID: &str = "T-1/R001";

fn fixture(
    entries: impl IntoIterator<Item = Value>,
    tail: &str,
) -> Result<(tempfile::TempDir, PathBuf)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("session.jsonl");
    let mut file = std::io::BufWriter::new(File::create(&path)?);
    for entry in entries {
        serde_json::to_writer(&mut file, &entry)?;
        file.write_all(b"\n")?;
    }
    file.write_all(tail.as_bytes())?;
    file.flush()?;
    Ok((dir, path))
}

#[test]
fn branch_cursor_skips_abandoned_entries_without_losing_pre_compaction_history() -> Result<()> {
    let mut entries = vec![
        json!({"type": "session", "version": 3}),
        json!({"type": "message", "id": "root", "parentId": null,
               "message": {"role": "user", "content": "original words"}}),
        json!({"type": "context_edit", "id": "hidden", "parentId": "root"}),
        json!({"type": "message", "id": "keep", "parentId": "hidden",
               "message": {"role": "assistant", "content": [{"type": "thinking", "thinking": "reason"}]}}),
    ];
    let mut parent = "root".to_owned();
    for index in 0..RECORDS - 1 {
        let id = format!("abandoned-{index}");
        entries.push(json!({"type": "message", "id": id, "parentId": parent,
                            "message": {"role": "assistant", "content": "abandoned"}}));
        parent = id;
    }
    entries.push(
        json!({"type": "compaction", "id": "leaf", "parentId": "keep",
                        "summary": "kept summary", "tokensBefore": 20}),
    );
    let (_dir, path) = fixture(entries, r#"{"id":"unfinished","parentId":"abandoned-0"}"#)?;
    let (latest, older) = page(ID, &path, None, None)?;
    assert!(
        matches!(latest.as_slice(), [message] if matches!(message.items.as_slice(),
            [SessionItem::Compaction { summary, tokens_before: 20 }] if summary == "kept summary"))
    );
    let (earlier, end) = page(
        ID,
        &path,
        None,
        Some(older.as_deref().context("expected older page")?),
    )?;
    assert!(end.is_none());
    let items: Vec<_> = earlier.iter().flat_map(|message| &message.items).collect();
    assert!(
        matches!(items.as_slice(), [SessionItem::Text { text }, SessionItem::Thinking { text: thinking }]
            if text == "original words" && thinking == "reason")
    );
    Ok(())
}

#[test]
fn large_foreign_record_pages_all_items_in_order() -> Result<()> {
    let blocks: Vec<_> = (0..8)
        .map(|index| {
            json!({"type": "output_text", "text": format!("{index}:{}", "界".repeat(6000))})
        })
        .collect();
    let (_dir, path) = fixture(
        [
            json!({"type": "user", "content": "before"}),
            json!({"type": "assistant", "content": blocks}),
        ],
        "",
    )?;
    assert!(fs::metadata(&path)?.len() > WINDOW as u64);
    let (latest, cursor) = page(ID, &path, None, None)?;
    let cursor = cursor.context("a split record needs an older cursor")?;
    let (earlier, end) = page(ID, &path, None, Some(&cursor))?;
    assert!(end.is_none());
    let mut items = earlier
        .iter()
        .chain(&latest)
        .flat_map(|message| &message.items);
    assert!(matches!(items.next(), Some(SessionItem::Text { text }) if text == "before"));
    for index in 0..8 {
        assert!(
            matches!(items.next(), Some(SessionItem::Text { text }) if text.starts_with(&format!("{index}:")))
        );
        assert!(matches!(
            items.next(),
            Some(SessionItem::Continuation { .. })
        ));
    }
    assert!(items.next().is_none());
    Ok(())
}

#[test]
fn paired_tool_continuation_preserves_result_and_authenticates_scope_and_content() -> Result<()> {
    let output = format!("x{}", "改".repeat(3200));
    let result = json!({"role": "toolResult", "toolCallId": "call-1", "toolName": "edit",
    "durationMs": 21, "details": {"diff": "-a\n+b"}, "content": [
        {"type": "text", "text": output},
        {"type": "image", "mimeType": "image/png", "data": "AA=="}
    ]});
    let (_dir, path) = fixture(
        [
            json!({"type": "message", "message": {"role": "assistant", "content": [
                {"type": "toolCall", "id": "call-1", "name": "edit", "arguments": {"path": "src/lib.rs"}}
            ]}}),
            json!({"type": "message", "message": result}),
        ],
        "",
    )?;
    let (messages, older) = page(ID, &path, None, None)?;
    assert!(older.is_none());
    let items: Vec<_> = messages.iter().flat_map(|message| &message.items).collect();
    let [
        preview,
        SessionItem::Continuation { token, bytes },
        SessionItem::Image { size: 1, .. },
    ] = items.as_slice()
    else {
        panic!("expected only a paired tool, its continuation and the result image");
    };
    assert!(matches!(
        preview,
        SessionItem::ToolCall { paired: true, .. }
    ));
    assert!(json_len(preview)? <= ITEM_BYTES);
    assert!(item("T-2/R001", &path, None, token).is_err());
    let mut tampered = token.clone();
    let last = tampered.pop().context("empty item token")?;
    tampered.push(if last == '0' { '1' } else { '0' });
    assert!(item(ID, &path, None, &tampered).is_err());

    let mut assembled = String::new();
    let mut next = Some(token.clone());
    while let Some(fragment) = next {
        let response = item(ID, &path, None, &fragment)?;
        assert_eq!(response.offset, assembled.len() as u64);
        assert_eq!(response.bytes, *bytes);
        assert!(response.json.len() <= 4096);
        assembled.push_str(&response.json);
        next = response.next;
    }
    assert_eq!(assembled.len() as u64, *bytes);
    let SessionItem::ToolCall {
        paired: true,
        state: SessionToolState::Done,
        elapsed_ms: Some(21),
        output: Some(full_output),
        result: Some(raw),
        diff: Some(diff),
        ..
    } = serde_json::from_str(&assembled)?
    else {
        panic!("expected a complete paired tool item");
    };
    assert_eq!(full_output, output);
    assert_eq!(diff, "-a\n+b");
    assert_eq!(serde_json::from_str::<Value>(&raw)?, result);

    // Keep record offsets intact, but change the content of the paired result.
    fs::write(&path, fs::read_to_string(&path)?.replacen('改', "界", 1))?;
    assert!(item(ID, &path, None, token).is_err());
    Ok(())
}

#[test]
fn result_without_call_and_aborted_call_keep_distinct_states() -> Result<()> {
    let (_dir, path) = fixture(
        [
            json!({"type": "message", "message": {"role": "toolResult", "toolCallId": "missing-call",
                                                   "content": "failed", "isError": true}}),
            json!({"type": "message", "message": {"role": "assistant", "stopReason": "aborted", "content": [
                {"type": "toolCall", "id": "next-call", "name": "bash", "arguments": {"command": "ls"}}
            ]}}),
        ],
        "",
    )?;
    let (messages, _) = page(ID, &path, None, None)?;
    assert!(matches!(&messages[0].items[0], SessionItem::ToolCall {
            state: SessionToolState::Failed, paired: false, call: None, name, arguments, ..
        } if name.is_empty() && arguments.is_empty()));
    assert!(matches!(
        &messages[1].items[0],
        SessionItem::ToolCall {
            state: SessionToolState::NotRun,
            paired: false,
            result: None,
            ..
        }
    ));
    assert!(matches!(&messages[1].items[1], SessionItem::Aborted { .. }));
    Ok(())
}

#[test]
fn transcript_symlink_is_refused() -> Result<()> {
    let (dir, path) = fixture([json!({"type": "user", "content": "recorded"})], "")?;
    let link = dir.path().join("link.jsonl");
    std::os::unix::fs::symlink(path, &link)?;
    assert!(page(ID, &link, None, None).is_err());
    Ok(())
}
