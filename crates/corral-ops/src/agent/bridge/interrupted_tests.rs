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
        "data":{"generation":generation,"session_id":session}})
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
        let scan = interrupted::dangling_calls(&path, generation, "s").unwrap();
        assert_eq!(
            scan.calls.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            [expected]
        );
        assert!(scan.limits.is_empty());
    }
    assert!(
        interrupted::dangling_calls(&path, 3, "s")
            .unwrap()
            .calls
            .is_empty()
    );
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
    let scan = interrupted::dangling_calls(&path, 2, "s").unwrap();
    assert_eq!(scan.calls[0].id, "old");
    assert_eq!(scan.limits, [interrupted::ScanLimit::GenerationBoundary]);
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
    for tool in ["edit", "write", "bash", "mystery"] {
        assert_eq!(
            interrupted::classify_upstream(tool, None).0,
            ResumeClass::OutcomeUnknown
        );
    }
    for tool in ["read", "grep", "find", "ls", "lsp_diagnostics"] {
        let class = interrupted::classify_upstream(tool, None).0;
        assert_eq!(class, ResumeClass::InterruptedNoEffect);
    }
}
