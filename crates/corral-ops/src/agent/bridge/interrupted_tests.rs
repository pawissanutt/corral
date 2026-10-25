//! Resume classes (T-252 recovery, F1.b): no class claims a call did not run without proof.
use corral_core::agent::{RequestReceipt, ResumeClass};
use serde_json::{Value, json};

use super::interrupted;

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
    assert!(interrupted::notice_body(1, 2, &[]).contains("No tool call"));
}
