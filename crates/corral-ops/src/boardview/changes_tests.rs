use super::*;
use corral_core::backend::{EventSeq, NewEvent};
use serde_json::json;

#[test]
fn kept_branch_approval_does_not_claim_integration() {
    let mut event = StoredEvent {
        seq: EventSeq(1),
        event: NewEvent {
            ts: "2026-11-01T00:00:00Z".parse().unwrap(),
            task: Some("T-001".into()),
            kind: "approved".into(),
            actor: None,
            via: None,
            run: Some("R1".into()),
            detail: json!({ "strategy": "keep_branch" }),
        },
        malformed: false,
    };
    let kept = change_line(&event);
    assert!(kept.text.contains("branch kept"));
    assert!(kept.text.contains("not merged"));
    assert!(!kept.text.contains("landed"));
    for strategy in ["fast_forward", "rebase"] {
        event.event.detail = json!({ "strategy": strategy });
        let integrated = change_line(&event);
        assert!(integrated.text.contains("landed"));
        assert!(!integrated.text.contains("not merged"));
    }
}
