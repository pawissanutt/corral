//! Resume classes (T-252 recovery, F1.b): no class claims a call did not run without proof.
use corral_core::agent::{RequestReceipt, ResumeClass};
use serde_json::{Value, json};

use super::interrupted;

#[test]
fn scan_uses_binding_identity_and_session_order_not_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let marker = |generation, session| {
        json!({"type":"custom", "customType":"corral_host_bound",
        "data":{"generation":generation,"session_id":session,"bind_nonce_sha256":"bind"}})
    };
    let call = |id| {
        json!({"type":"message", "message":{"role":"assistant",
        "content":[{"type":"toolCall","id":id,"name":"edit"}]}})
    };
    let entries = [
        marker(1, "other-session"),
        call("wrong-session"),
        marker(1, "s"),
        call("g1"),
        marker(2, "s"),
        call("g2"),
    ];
    std::fs::write(
        &path,
        entries.iter().map(|e| format!("{e}\n")).collect::<String>(),
    )
    .unwrap();
    for (generation, expected) in [(1, "g1"), (2, "g2")] {
        let scan = interrupted::dangling_calls(&path, generation, "s", "bind").unwrap();
        assert_eq!(
            scan.calls.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            [expected]
        );
        assert!(scan.limits.is_empty());
    }
    assert!(
        interrupted::dangling_calls(&path, 3, "s", "bind")
            .unwrap()
            .calls
            .is_empty()
    );
}

fn continued_review_child(target: u32) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("continued.jsonl");
    let marker = |generation, binding| {
        json!({"type":"custom", "customType":"corral_host_bound",
        "data":{"generation":generation,"session_id":"continued-review",
            "bind_nonce_sha256":binding}})
    };
    let call = |id| {
        json!({"type":"message", "timestamp":"2020-01-01T00:00:00Z",
        "message":{"role":"assistant", "content":[{"type":"toolCall","id":id,"name":"edit"}]}})
    };
    // Child A recovers and finishes; child B continues the same conversation, restarting at g1.
    // Keep old unmatched calls too: recovery must neither lose B's calls nor report A's calls.
    let entries = [
        marker(1, "a1"),
        call("old-a1"),
        marker(2, "a2"),
        call("old-a2"),
        marker(1, "b1"),
        call("current-b1"),
        marker(2, "b2"),
        call("current-b2"),
    ];
    std::fs::write(
        &path,
        entries.iter().map(|e| format!("{e}\n")).collect::<String>(),
    )
    .unwrap();
    let scan =
        interrupted::dangling_calls(&path, target, "continued-review", &format!("b{target}"))
            .unwrap();
    let expected = format!("current-b{target}");
    assert_eq!(
        scan.calls.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        [expected.as_str()]
    );
    assert!(scan.limits.is_empty());
}

#[test]
fn continued_review_initial_generation_does_not_alias_an_older_child() {
    continued_review_child(1);
}

#[test]
fn continued_review_resumed_generation_does_not_alias_an_older_child() {
    continued_review_child(2);
}

#[test]
fn legacy_session_without_binding_markers_is_conservative_and_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.jsonl");
    std::fs::write(
        &path,
        format!(
            "{}\n",
            json!({"type":"message", "message":{
        "role":"assistant", "content":[{"type":"toolCall","id":"old","name":"bash"}]}})
        ),
    )
    .unwrap();
    let legacy = std::fs::read_to_string(&path).unwrap();
    // Pre-fix markers without a binding digest also cannot safely scope a reused session.
    let weak_marker = json!({"type":"custom", "customType":"corral_host_bound",
        "data":{"generation":1,"session_id":"s"}});
    for prefix in [String::new(), format!("{weak_marker}\n")] {
        std::fs::write(&path, format!("{prefix}{legacy}")).unwrap();
        let scan = interrupted::dangling_calls(&path, 2, "s", "bind").unwrap();
        assert_eq!(scan.calls[0].id, "old");
        assert_eq!(scan.limits, [interrupted::ScanLimit::GenerationBoundary]);
    }
}

#[test]
fn classification_never_claims_not_executed_without_proof() {
    let gate = |state: &str, result: Value| -> RequestReceipt {
        serde_json::from_value(json!({"version": 1, "request_id": "rq", "op": "gate",
            "payload_sha256": "00", "identity": {"store": "s", "project": "p", "role": "worker",
            "session_id": "s", "generation": 1}, "state": state, "result": result,
            "at": "2026-10-25T00:00:00Z"}))
        .unwrap()
    };
    let class = |op: &str, receipt: Option<&RequestReceipt>, running: bool| {
        interrupted::classify_corral(op, receipt, running).0
    };
    let prepared = gate(
        "reserved",
        json!({"outcome": "prepared", "attempt": "ag-1"}),
    );
    let uncertain = gate("uncertain", json!({"outcome": "outcome_unknown"}));
    let not_started = gate("abandoned", json!({"outcome": "not_started"}));
    let captured = gate("committed", json!({"outcome": "captured", "task": "T-9"}));
    assert_eq!(
        class("gate", Some(&prepared), true),
        ResumeClass::InProgress
    );
    assert_eq!(class("gate", Some(&prepared), false), ResumeClass::Prepared);
    assert_eq!(
        class("reply", Some(&uncertain), false),
        ResumeClass::OutcomeUnknown
    );
    assert_eq!(
        class("capture", Some(&not_started), false),
        ResumeClass::NotExecuted
    );
    assert_eq!(
        class("capture", Some(&captured), false),
        ResumeClass::Result
    );
    assert_eq!(class("capture", None, false), ResumeClass::NotExecuted);
    assert_eq!(class("comment", None, false), ResumeClass::NotCommitted);
    for tool in ["edit", "write", "bash", "mystery", "lsp_diagnostics"] {
        assert_eq!(
            interrupted::classify_upstream(tool, None).0,
            ResumeClass::OutcomeUnknown
        );
    }
    for tool in ["read", "grep", "find", "ls"] {
        let class = interrupted::classify_upstream(tool, None).0;
        assert_eq!(class, ResumeClass::InterruptedNoEffect);
    }
}

#[test]
fn hook_attribution_makes_possibly_affected_reads_and_unfinished_hooks_unknown() {
    // Given a lost generation whose session recorded hooks around reads and at turn ends.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hooks.jsonl");
    let hook = |invocation: &str, phase: &str, call: Option<&str>, turn: u64, state: &str| {
        json!({"type":"custom", "customType": interrupted::HOOK_ENTRY, "data": {
            "invocation": invocation, "phase": phase, "hook": "touch", "tool_call_id": call,
            "turn": turn, "state": state}})
    };
    let call = |id: &str, name: &str| {
        json!({"type":"message", "message":{"role":"assistant",
        "content":[{"type":"toolCall","id":id,"name":name}]}})
    };
    let result =
        |id: &str| json!({"type":"message", "message":{"role":"toolResult","toolCallId":id}});
    let entries = [
        json!({"type":"custom", "customType":"corral_host_bound",
            "data":{"generation":1,"session_id":"s","bind_nonce_sha256":"bind"}}),
        call("done-read", "read"),
        hook("hk-1-1", "pre_tool", Some("done-read"), 1, "prepared"),
        hook("hk-1-1", "pre_tool", Some("done-read"), 1, "completed"),
        result("done-read"),
        hook("hk-1-2", "turn_end", None, 1, "started"),
        hook("hk-1-2", "turn_end", None, 1, "completed"),
        call("hooked-read", "read"),
        hook("hk-1-3", "pre_tool", Some("hooked-read"), 2, "started"),
        call("plain-read", "grep"),
        call("edit-call", "edit"),
        call("todo", "corral_todo"),
        hook("hk-1-4", "turn_end", None, 2, "started"),
    ];
    std::fs::write(
        &path,
        entries.iter().map(|e| format!("{e}\n")).collect::<String>(),
    )
    .unwrap();
    // When the scan runs, then completed hooks of finished calls and turns are pruned.
    let scan = interrupted::dangling_calls(&path, 1, "s", "bind").unwrap();
    let invocations: Vec<&str> = scan.hooks.iter().map(|h| h.invocation.as_str()).collect();
    assert_eq!(invocations, ["hk-1-3", "hk-1-4"]);
    // And a recorded hook makes its read unknown; a read no hook recorded or wraps stays no-effect.
    let settings = corral_core::agent::HookSettings {
        pre_tool: vec![corral_core::agent::HookCommand {
            name: "touch".into(),
            argv: vec!["touch".into()],
            tools: vec!["read".into()],
        }],
        ..Default::default()
    };
    let classify = |id: &str, settings| {
        let call = scan.calls.iter().find(|c| c.id == id).unwrap();
        interrupted::classify_local(call, (Some("saved.diff"), &scan.hooks, settings))
    };
    let (class, detail) = classify("hooked-read", None);
    assert_eq!(class, ResumeClass::OutcomeUnknown);
    let detail = detail.unwrap();
    assert!(detail.contains("hk-1-3 (pre_tool touch, started)") && detail.contains("saved.diff"));
    assert_eq!(
        classify("plain-read", None),
        (ResumeClass::InterruptedNoEffect, None)
    );
    assert_eq!(
        classify("plain-read", Some(&settings)).0,
        ResumeClass::InterruptedNoEffect
    );
    // And configured hooks that wrap a tool make it unknown even without a surviving record.
    let wrapped = interrupted::DanglingCall {
        id: "lost".into(),
        name: "read".into(),
    };
    let (class, detail) =
        interrupted::classify_local(&wrapped, (None, &scan.hooks, Some(&settings)));
    assert_eq!(class, ResumeClass::OutcomeUnknown);
    assert!(detail.unwrap().contains("may have run"));
    assert_eq!(classify("edit-call", None).0, ResumeClass::OutcomeUnknown);
    // And the interrupted turn-end hook is reported unknown by invocation, never as a replay.
    let unfinished = interrupted::unfinished_hooks(&scan);
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].tool_call_id, "hk-1-4");
    assert_eq!(unfinished[0].tool, "hook:turn_end:touch");
    assert_eq!(unfinished[0].class, ResumeClass::OutcomeUnknown);
    let note = unfinished[0].detail.as_deref().unwrap();
    assert!(note.contains("for turn 2") && note.contains("does not replay"));
    // And Corral calls are never hook-attributed: no hook names them, so receipts decide.
    assert!(
        scan.hooks
            .iter()
            .all(|h| h.tool_call_id.as_deref() != Some("todo"))
    );
    assert!(!settings.wraps("corral_todo"));
}
