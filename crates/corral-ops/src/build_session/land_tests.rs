use super::*;
use crate::build_session::gate::tests::{Fixture, git};

fn land(fixture: &Fixture, attempt: &str) -> Result<LandResult> {
    run(
        &fixture.ctx,
        "B-fixture",
        &fixture.worktree,
        "app",
        &fixture.head,
        attempt,
    )
}

#[test]
fn matching_full_gate_fast_forwards_and_records_workspace_landing() {
    let fixture = Fixture::new(&["true"]);
    let receipt = fixture.gate().unwrap();
    // Board landing permits untracked files in the main checkout.
    std::fs::write(fixture.repo.join("untracked-owner-file"), "keep\n").unwrap();
    let result = land(&fixture, &receipt.attempt).unwrap();
    assert_eq!(result.landed, fixture.head);
    assert_eq!(result.previous, fixture.base);
    assert_eq!(result.base_branch, "main");
    assert_eq!(
        corral_git::rev_sha(&fixture.repo, "main").unwrap(),
        fixture.head
    );
    let events = fixture.ctx.store.events().unwrap();
    let event = events
        .iter()
        .find(|event| event.kind == "build_landed")
        .unwrap();
    assert_eq!(event.task, None);
    assert_eq!(
        event.detail,
        serde_json::json!({
            "session": "B-fixture", "project": "app", "head": fixture.head,
            "landed": fixture.head, "previous": fixture.base,
        })
    );
}

#[test]
fn receipt_for_another_head_cannot_authorize_landing() {
    let fixture = Fixture::new(&["true"]);
    let mut receipt = fixture.gate().unwrap();
    receipt.head = fixture.base.clone();
    std::fs::write(&receipt.receipt, serde_json::to_vec(&receipt).unwrap()).unwrap();
    assert!(
        land(&fixture, &receipt.attempt)
            .unwrap_err()
            .to_string()
            .starts_with("gate_stale:")
    );
    assert_eq!(
        corral_git::rev_sha(&fixture.repo, "main").unwrap(),
        fixture.base
    );
}

#[test]
fn full_gate_cannot_land_a_candidate_missing_the_current_base_head() {
    let fixture = Fixture::new(&["true"]);
    let receipt = fixture.gate().unwrap();
    std::fs::write(fixture.repo.join("owner-change"), "base advanced\n").unwrap();
    corral_git::commit_all(&fixture.repo, "advance base").unwrap();
    let advanced = corral_git::rev_sha(&fixture.repo, "main").unwrap();
    assert!(
        land(&fixture, &receipt.attempt)
            .unwrap_err()
            .to_string()
            .starts_with("landing_stale:")
    );
    assert_eq!(
        corral_git::rev_sha(&fixture.repo, "main").unwrap(),
        advanced
    );
}

#[test]
fn landing_keeps_the_boards_main_checkout_precondition_order() {
    let fixture = Fixture::new(&["true"]);
    let receipt = fixture.gate().unwrap();
    git(&fixture.repo, &["switch", "-qc", "owner-branch"]);
    std::fs::write(fixture.repo.join("source"), "dirty owner source\n").unwrap();
    assert!(
        land(&fixture, &receipt.attempt)
            .unwrap_err()
            .to_string()
            .starts_with("main_off_base:")
    );
    git(&fixture.repo, &["switch", "main"]);
    assert!(
        land(&fixture, &receipt.attempt)
            .unwrap_err()
            .to_string()
            .starts_with("main_dirty:")
    );
    assert_eq!(
        corral_git::rev_sha(&fixture.repo, "main").unwrap(),
        fixture.base
    );
}
