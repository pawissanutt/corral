//! Owner changes supersede the gate phase, not the historical approval or receipts.
use corral_core::{GateResult, SessionPolicy, Status, Verdict};
use corral_ops::GatePhase;

use super::{Fx, SUMMARY, quick_command};

fn approved(fx: &Fx) -> String {
    let id = fx.create("Approved candidate returned for changes");
    fx.audited(&id);
    fx.dispatch(&id).unwrap();
    fx.commit(&id, "original.txt");
    assert_eq!(fx.gate(&id).result, GateResult::Pass);
    fx.ctx
        .store
        .update(&id, |task| {
            task.status = Status::Review;
            Ok(())
        })
        .unwrap();
    corral_ops::verdict(&fx.ctx, &id, Verdict::Approve, "approved").unwrap();
    let task = fx.ctx.store.get(&id).unwrap();
    let contract = corral_ops::gate_readback(&fx.ctx, &task)
        .unwrap()
        .gate_contract
        .unwrap();
    assert_eq!(contract.phase, GatePhase::Landing);
    assert_eq!(contract.tier, "full");
    id
}

fn returned_for_changes(policy: SessionPolicy, change_candidate: bool) {
    // Given an approved run with its native review record and quick receipt.
    let fx = Fx::new(true, |store| {
        store.config.review.worker_session = Some(policy);
    });
    let id = approved(&fx);
    let before = fx.run(&id);
    let receipt = before.gate.as_ref().unwrap();
    let approval = before.review.as_ref().unwrap();

    // When the owner reopens that same run through the native changes transition.
    let task = corral_ops::request_changes(&fx.ctx, &id, "address the finding").unwrap();

    // Then history stays intact, but the contract is review/quick.
    assert_eq!(task.status, Status::Running);
    let reopened = task.current_run().unwrap();
    assert_eq!(reopened.id, before.id);
    assert_eq!(reopened.review.as_ref(), Some(approval));
    assert_eq!(reopened.gate.as_ref(), Some(receipt));
    let round = reopened.changes_round.as_ref().unwrap();
    assert_eq!(round.n, 1);
    assert_eq!(round.sha, approval.sha);
    assert!(round.at > approval.at);
    let readback = corral_ops::gate_readback(&fx.ctx, &task).unwrap();
    let contract = readback.gate_contract.unwrap();
    assert_eq!(contract.phase, GatePhase::Review);
    assert_eq!(contract.tier, "quick");
    assert_eq!(contract.source, "policy:review_rounds");
    assert_eq!(contract.commands, [quick_command(&fx.out)]);
    assert_eq!(
        readback.gate_ref.unwrap().attempt,
        receipt.identity.as_ref().unwrap().attempt
    );
    if change_candidate {
        fx.commit(&id, "finding.txt");
        assert_ne!(
            fx.run(&id).review.as_ref().unwrap().sha,
            super::git(&fx.run(&id).worktree, &["rev-parse", "HEAD"])
        );
    }

    // The actual launch and done audit agree with native readback, even after a new HEAD.
    let gate = fx.gate(&id);
    assert_eq!(gate.result, GateResult::Pass, "{:?}", gate.failed_cmd);
    assert_eq!(gate.tier.as_ref().unwrap().tier, "quick");
    assert_eq!(fx.runs(), ["quick:quick", "quick:quick"]);
    fx.observe(&id, &gate.identity.as_ref().unwrap().attempt);
    fx.checkpoint(&id);
    let audit = corral_ops::completion_check(&fx.ctx, &id, SUMMARY).unwrap();
    assert!(audit.passed, "{:?}", audit.failures);
    assert_eq!(audit.required_checks, contract.commands);
    let task = fx.ctx.store.get(&id).unwrap();
    let readback = corral_ops::gate_readback(&fx.ctx, &task).unwrap();
    assert_eq!(readback.gate_contract.as_ref(), Some(&contract));
    assert_eq!(
        readback.gate_ref.unwrap().attempt,
        gate.identity.as_ref().unwrap().attempt
    );
    assert_eq!(fx.run(&id).review, before.review);
    let starts = fx.events(&id, "gate_started");
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0]["identity"], serde_json::json!(receipt.identity));
}

#[test]
fn owner_changes_after_approval_reopen_review_gate() {
    returned_for_changes(SessionPolicy::Reuse, false);
}

#[test]
fn owner_changes_after_approval_keep_review_gate_after_candidate_changes() {
    returned_for_changes(SessionPolicy::Reuse, true);
}

#[test]
fn owner_changes_after_approval_reopen_review_gate_with_fresh_worker_session() {
    returned_for_changes(SessionPolicy::Fresh, true);
}

#[test]
fn approval_after_backward_wall_step_still_resolves_changes_round() {
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };
    let ahead = corral_core::clock::now() + jiff::SignedDuration::from_hours(1);
    let wall = Arc::new(AtomicI64::new(ahead.as_second()));
    let source = Arc::clone(&wall);
    corral_core::clock::with_test_wall(
        move || jiff::Timestamp::from_second(source.load(Ordering::SeqCst)).unwrap(),
        || {
            let fx = Fx::new(true, |_| {});
            let id = approved(&fx);
            corral_ops::request_changes(&fx.ctx, &id, "address the finding").unwrap();
            fx.commit(&id, "finding.txt");
            let gate = fx.gate(&id);
            fx.observe(&id, &gate.identity.as_ref().unwrap().attempt);
            fx.checkpoint(&id);
            corral_ops::done(&fx.ctx, &id, SUMMARY).unwrap();
            let round = fx.run(&id).changes_round.unwrap().at;
            wall.fetch_sub(7200, Ordering::SeqCst);
            let approval =
                corral_ops::verdict(&fx.ctx, &id, Verdict::Approve, "addressed").unwrap();
            assert!(approval.at > round);
            let task = fx.ctx.store.get(&id).unwrap();
            assert_eq!(
                corral_ops::gate_readback(&fx.ctx, &task)
                    .unwrap()
                    .gate_contract
                    .unwrap()
                    .phase,
                GatePhase::Landing
            );
        },
    );
}

#[test]
fn renewed_approval_after_owner_changes_restores_landing_gate() {
    // Given a changes round and a fresh quick receipt on its changed candidate.
    let fx = Fx::new(true, |_| {});
    let id = approved(&fx);
    corral_ops::request_changes(&fx.ctx, &id, "address the finding").unwrap();
    fx.commit(&id, "finding.txt");
    let gate = fx.gate(&id);
    assert_eq!(gate.tier.as_ref().unwrap().tier, "quick");
    fx.observe(&id, &gate.identity.as_ref().unwrap().attempt);
    fx.checkpoint(&id);
    corral_ops::done(&fx.ctx, &id, SUMMARY).unwrap();

    // When the owner approves the new candidate.
    corral_ops::verdict(&fx.ctx, &id, Verdict::Approve, "finding addressed").unwrap();

    // Then the retained changes round no longer masks landing/full.
    let task = fx.ctx.store.get(&id).unwrap();
    let contract = corral_ops::gate_readback(&fx.ctx, &task)
        .unwrap()
        .gate_contract
        .unwrap();
    assert_eq!(contract.phase, GatePhase::Landing);
    assert_eq!(contract.tier, "full");
    assert_eq!(fx.gate(&id).tier.as_ref().unwrap().tier, "full");
}
