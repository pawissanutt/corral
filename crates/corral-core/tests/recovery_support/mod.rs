use corral_core::{RecoveryRecord, RecoveryState, Store};
use serde_json::json;

pub fn recovery(store: &Store) -> RecoveryRecord {
    serde_json::from_value(json!({
        "version": 1,
        "id": "REC1",
        "preview": {
            "version": 1,
            "request": {
                "version": 1,
                "target": {
                    "store": store.root(),
                    "task": "T-001",
                    "task_revision": "task-v1",
                    "spec_revision": "spec-v1",
                    "identity": {
                        "kind": "plan", "plan": "P1",
                        "artifact_sha256": "original-plan-sha",
                        "review_attempt": "PR1"
                    }
                },
                "action": {
                    "action": "use-reviewer", "stage": "plan-review",
                    "reviewer": {"profile": "astra", "model": "fixture/astra", "vendor": "fixture"}
                },
                "exceptions": ["reviewer-family-policy", "configured-reviewer-binding"]
            },
            "policy": {
                "digest": "unmodified-policy-digest",
                "inputs": {"require_cross_family": true, "reviewer": "sol"},
                "sources": {"require_cross_family": "project", "reviewer": "project"},
                "expected_reviewer": {"profile": "sol", "model": "fixture/sol", "vendor": "fixture"},
                "resolution_failure": {"code": "reviewer_unavailable", "message": "unavailable", "evidence": ["route-log"]}
            },
            "checks": [
                {"check": "reviewer-family-policy", "observed": {"kind": "predicate", "satisfied": false},
                    "disposition": "overridden", "evidence": ["family-error"]},
                {"check": "configured-reviewer-binding", "observed": {"kind": "predicate", "satisfied": false},
                    "disposition": "overridden", "evidence": ["expected-sol"]}
            ],
            "blockers": [{
                "code": "reviewer-family-policy", "class": "recoverable",
                "observed": {"kind": "predicate", "satisfied": false},
                "evidence": ["family-error"], "proposed_disposition": "overridden",
                "ordinary_command": null
            }],
            "remaining_blockers": [],
            "proposed_effect": {
                "kind": "review_attempt", "stage": "plan-review", "attempt": "PR2",
                "reviewer": {"profile": "astra", "model": "fixture/astra", "vendor": "fixture"}
            },
            "affected_targets": ["T-001"],
            "expires": "2026-10-18T12:00:00Z",
            "digest": "exact-preview-digest",
            "contract_digest": "bound-recovery-contract"
        },
        "authorization": {
            "version": 1, "preview_digest": "exact-preview-digest",
            "reason": "Use this separate Astra review attempt only",
            "attestation": {
                "kind": "local_owner_attestation", "interaction": "cli",
                "at": "2026-10-18T11:00:00Z",
                "controlling_terminal": "/dev/pts/fixture", "context": null
            }
        },
        "outcomes": [{
            "state": "authorized", "at": "2026-10-18T11:00:00Z",
            "detail": "intent committed", "actual_effect": null, "after": null,
            "evidence": []
        }],
        "links": []
    })).unwrap()
}

pub fn append_outcome(record: &mut RecoveryRecord, state: RecoveryState) {
    let mut outcome = record.outcomes[0].clone();
    outcome.state = state;
    outcome.detail = format!("{state:?}");
    record.outcomes.push(outcome);
}
