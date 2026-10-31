use corral_core::{ChangesRound, Run, Verdict};
use serde_json::json;

use super::{GatePhase, current_phase};

#[test]
fn phase_follows_approval_order_without_comparing_candidate_or_receipt_identity() {
    // Given an approval with explicit stamps, independent of any filesystem or wall clock.
    let approved: Run = serde_json::from_value(json!({
        "id": "R1", "agent": "fake", "worktree": "/unused",
        "branch": "corral/T-1", "base": "main", "base_sha": "base",
        "started": "2026-10-23T00:00:00Z",
        "review": {"verdict": "approve", "reviewer": "owner",
                   "sha": "approved-head", "at": "2026-10-23T01:00:00Z"}
    }))
    .unwrap();
    let changes: ChangesRound = serde_json::from_value(json!({
        "n": 1, "sha": "another-head", "at": "2026-10-23T02:00:00Z"
    }))
    .unwrap();
    let review_at = approved.review.as_ref().unwrap().at;
    for (round, phase) in [
        (None, GatePhase::Landing),
        (
            Some(ChangesRound {
                at: "2026-10-23T00:30:00Z".parse().unwrap(),
                ..changes.clone()
            }),
            GatePhase::Landing,
        ),
        (
            Some(ChangesRound {
                at: review_at,
                ..changes.clone()
            }),
            GatePhase::Review,
        ),
        (Some(changes), GatePhase::Review),
    ] {
        let run = Run {
            changes_round: round,
            ..approved.clone()
        };
        // When choosing the phase, only an approval newer than the changes round lands.
        assert_eq!(
            current_phase(&corral_core::RunHead::of(&run).unwrap()),
            phase
        );
    }
}

#[test]
fn phase_stays_review_without_an_approval() {
    for verdict in [None, Some(Verdict::Changes), Some(Verdict::Escalate)] {
        // Given a pending or non-approving review.
        let run: Run = serde_json::from_value(json!({
            "id": "R1", "agent": "fake", "worktree": "/unused",
            "branch": "corral/T-1", "base": "main", "base_sha": "base",
            "started": "2026-10-23T00:00:00Z",
            "review": {"verdict": verdict, "reviewer": "owner",
                       "sha": "head", "at": "2026-10-23T01:00:00Z"}
        }))
        .unwrap();
        // When choosing the phase, no non-approval can authorize landing.
        assert_eq!(
            current_phase(&corral_core::RunHead::of(&run).unwrap()),
            GatePhase::Review
        );
    }
}
