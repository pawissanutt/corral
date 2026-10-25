//! T-308 review round 1: a reviewer's own corral_evidence and corral_artifact are reviewer-side
//! proof. Each commits through an authenticated envelope, and the same child's verdict still
//! commits, while a genuine candidate change after the review started is still refused.
use corral_core::{CriteriaManifest, Criterion, EvidenceObservation, ObservationStatus};

use super::*;

/// A review of a task whose manifest registers criterion C1, with one candidate proof artifact.
fn review_with_criteria() -> (Review, String) {
    let mut proof_ref = String::new();
    let review = review_with(|ctx, id| {
        let spec_digest = crate::completion_spec_digest(&ctx.store.get(id).unwrap()).unwrap();
        let manifest = CriteriaManifest {
            version: 1,
            spec_digest,
            reviewed_by: "owner".into(),
            audited: false,
            require_gate: false,
            owner_decisions: vec![],
            criteria: vec![Criterion {
                id: "C1".into(),
                required: true,
                live: false,
                scenario: "fixture".into(),
                expected: "zero failures".into(),
            }],
        };
        crate::register_criteria(ctx, id, &manifest).unwrap();
        let proof = ctx.store.root().join("proof.log");
        std::fs::write(&proof, "candidate proof\n").unwrap();
        proof_ref = crate::add_artifact(ctx, id, &proof, None, None)
            .unwrap()
            .path;
    });
    (review, proof_ref)
}

fn observation(proof_ref: &str) -> Value {
    serde_json::to_value(EvidenceObservation {
        version: 1,
        key: "review-observation".into(),
        criterion: "C1".into(),
        run: "R1".into(),
        attempt: "no-gate".into(),
        status: ObservationStatus::Pass,
        action: "checked the fixture".into(),
        environment: "isolated".into(),
        expected: "zero failures".into(),
        observed: "zero failures".into(),
        artifacts: vec![proof_ref.to_owned()],
        captured: "2026-10-25T00:00:00Z".parse().unwrap(),
        producer: "reviewer".into(),
        live: false,
        cleanup: None,
    })
    .unwrap()
}

fn changes(review: &Review, call: &str) -> CallResponse {
    review.op(
        Some(1),
        call,
        "verdict",
        json!({"verdict": "changes", "notes": "Changes requested."}),
    )
}

/// The reviewer-side artifact of a committed receipt: attached to the parent run, its source
/// under review-watch, and its staging directory gone.
fn assert_reviewer_side(review: &Review, receipt: &corral_core::agent::RequestReceipt) {
    let task = review.task();
    let path = receipt.result["path"].as_str().unwrap();
    let artifact = task.artifacts.iter().find(|a| a.path == path).unwrap();
    assert_eq!(artifact.run, "R1");
    let watch = review.ctx().store.state_dir().join("review-watch");
    assert!(
        Path::new(&artifact.source).starts_with(&watch),
        "{}",
        artifact.source
    );
    assert!(
        !Path::new(&artifact.source).exists(),
        "the staged copy is removed"
    );
}

#[test]
fn reviewer_evidence_then_verdict_both_commit_and_leave_the_worker_evidence_unchanged() {
    let (mut review, proof_ref) = review_with_criteria();
    committed(&review.event("turn_started", 1));
    let before = review.task().runs[0].completion.evidence.clone();
    let input = review.envelope(
        Some(1),
        "call-evidence",
        "evidence",
        observation(&proof_ref),
    );
    let recorded = committed(&send(&review.fx, &input, "evidence"));
    assert_eq!(recorded.result["added"], true);
    assert_reviewer_side(&review, &recorded);
    assert_eq!(review.task().runs[0].completion.evidence, before);
    assert_eq!(
        committed(&changes(&review, "call-verdict")).result["verdict"],
        "changes"
    );
    // A retry of the evidence request after the verdict replays its stored receipt.
    match send(&review.fx, &input, "evidence") {
        CallResponse::Receipt { receipt, replayed } => {
            assert!(replayed);
            assert_eq!(receipt, recorded);
        }
        other => panic!("expected the stored evidence receipt, got {other:?}"),
    }
}

#[test]
fn a_new_reviewer_artifact_then_verdict_both_commit_and_replay_without_the_source_file() {
    let (mut review, _) = review_with_criteria();
    committed(&review.event("turn_started", 1));
    let app = review.task().runs[0].worktree.clone();
    let fresh = review.ctx().store.root().join("reviewer-proof.log");
    std::fs::write(&fresh, "new reviewer proof\n").unwrap();
    let payload = json!({"path": fresh, "kind": "log", "title": "Reviewer observation"});
    let input = review.envelope(Some(1), "call-artifact", "artifact", payload);
    let attached = committed(&send(&review.fx, &input, "artifact"));
    assert_eq!(attached.result["added"], true);
    assert_reviewer_side(&review, &attached);
    assert_eq!(git(&app, &["status", "--porcelain"]), "");
    assert_eq!(
        committed(&changes(&review, "call-verdict")).result["verdict"],
        "changes"
    );
    std::fs::remove_file(&fresh).unwrap();
    match send(&review.fx, &input, "artifact") {
        CallResponse::Receipt { receipt, replayed } => {
            assert!(replayed);
            assert_eq!(receipt, attached);
        }
        other => panic!("expected the stored artifact receipt, got {other:?}"),
    }
}

#[test]
fn a_candidate_artifact_added_after_the_review_started_still_stales_the_verdict() {
    let (mut review, _) = review_with_criteria();
    committed(&review.event("turn_started", 1));
    let fresh = review.ctx().store.root().join("reviewer-proof.log");
    std::fs::write(&fresh, "reviewer proof\n").unwrap();
    let payload = json!({"path": fresh, "title": "Reviewer observation"});
    committed(&review.op(Some(1), "call-artifact", "artifact", payload));
    let late = review.ctx().store.root().join("late-candidate.log");
    std::fs::write(&late, "a new worker artifact\n").unwrap();
    crate::add_artifact(review.ctx(), &review.id, &late, None, None).unwrap();
    let (code, reason) = rejection(&changes(&review, "call-verdict"));
    assert_eq!(code, "review_closed");
    assert!(reason.contains("packet_digest"), "{reason}");
}
