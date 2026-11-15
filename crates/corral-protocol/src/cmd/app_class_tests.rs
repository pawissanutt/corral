use super::*;
use crate::app::ActClass;

#[test]
fn d18_confirms_landing_acceptance_revert_and_finish_not_ordinary_instructions() {
    let task = |args| serde_json::json!({"family": "task", "args": args});
    let review = |args| serde_json::json!({"family": "review", "args": args});
    let cases = [
        (
            task(serde_json::json!({
                "verb": "retry", "id": "T-1", "run": null, "profile": null
            })),
            ActClass::Act,
        ),
        (
            task(serde_json::json!({
                "verb": "idea", "id": "T-1", "action": "accept", "reason": "",
                "into": null, "body": null, "owner_ok": true
            })),
            ActClass::Act,
        ),
        (
            task(serde_json::json!({
                "verb": "move", "id": "T-1", "status": "ready", "force": false,
                "owner_ok": true, "reason": null
            })),
            ActClass::Act,
        ),
        (
            task(serde_json::json!({
                "verb": "move", "id": "T-1", "status": "done", "force": false,
                "owner_ok": true, "reason": null
            })),
            ActClass::Confirm,
        ),
        (
            task(serde_json::json!({"verb": "revert", "id": "T-1"})),
            ActClass::Confirm,
        ),
        (
            review(serde_json::json!({
                "verb": "verdict", "id": "T-1", "sha": "candidate", "verdict": "pass",
                "notes": null
            })),
            ActClass::Act,
        ),
        (
            review(serde_json::json!({
                "verb": "request-changes", "id": "T-1", "text": "", "escalate": false
            })),
            ActClass::Act,
        ),
        (
            review(serde_json::json!({
                "verb": "approve", "id": "T-1", "sha": "candidate"
            })),
            ActClass::Confirm,
        ),
        (
            serde_json::json!({"family": "plan", "args": {
                "verb": "accept", "id": "P-1", "revision": "1", "owner_ok": true
            }}),
            ActClass::Confirm,
        ),
    ];
    for (value, expected) in cases {
        let command: Command = serde_json::from_value(value).unwrap();
        assert_eq!(command.app_class(), expected, "{}", command.name());
    }
}
