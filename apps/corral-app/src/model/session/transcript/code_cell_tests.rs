use corral_protocol::detail::{SessionItem, SessionToolState};

use super::group_tests::{feed_with, said, shown};
use super::{CellRow, MessageItem, ToolState};

#[test]
fn a_cell_lists_the_calls_it_recorded_not_its_script() {
    // The script names three reads; the runtime recorded one bash call.
    let script = "const a = await read({ path: 'a.rs' });\nconst b = await read({ path: 'b.rs' });\nawait read({ path: 'c.rs' });";
    let arguments = serde_json::json!({ "code": script }).to_string();
    let result = serde_json::json!({
        "role": "toolResult",
        "toolCallId": "m1",
        "toolName": "codemode",
        "content": [{ "type": "text", "text": "cell-3 ok" }],
        "details": { "cells": [{
            "cell": "cell-3",
            "status": "ok",
            "stateLost": false,
            "outputs": [],
            "calls": { "total": 1, "ok": 1, "failed": 0, "running": 0 },
            "failures": [],
            "children": [{
                "tool": "bash",
                "call": "k1",
                "key": "k1",
                "args": "{\"command\":\"cargo test\"}",
                "startedAt": 0,
                "durationMs": 40,
                "ok": true,
                "preview": "test result: ok. 9 passed"
            }],
            "startedAt": 0,
            "durationMs": 1200
        }]}
    })
    .to_string();
    let feed = feed_with(vec![said(
        "assistant",
        "2026-11-15T10:00:00Z",
        vec![SessionItem::ToolCall {
            call_id: "m1".into(),
            name: "codemode".into(),
            arguments,
            summary: String::new(),
            state: SessionToolState::Done,
            paired: true,
            call: None,
            output: Some("cell-3 ok".into()),
            result: Some(result),
            elapsed_ms: Some(1200),
            diff: None,
            content: None,
        }],
    )]);
    let page = shown(&feed);
    let [MessageItem::Cell(cell)] = page.messages[0].items.as_slice() else {
        panic!("the codemode call is drawn as its cell");
    };
    let calls: Vec<&str> = cell
        .children
        .iter()
        .map(|row| match row {
            CellRow::Call(call) => call.name.as_str(),
            CellRow::Fold { .. } => "fold",
        })
        .collect();
    assert_eq!(calls, ["bash"]);
    assert_eq!(cell.state, ToolState::Ok);
    assert_eq!(cell.tally, [(ToolState::Ok, 1)]);
}
