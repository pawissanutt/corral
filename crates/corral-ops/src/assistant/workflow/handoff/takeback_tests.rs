use super::plan_adoption_tests::{board, instruction};
use super::*;
use crate::agent::test_support as ts;
use corral_core::TaskRef as _;

#[test]
fn shared_pm_review_stays_held_and_uncancelled_through_save_takeback_and_reopen() {
    let (f, session, task) = board();
    let task = f
        .ctx
        .store
        .update(&task.id, |task| {
            task.plans[0]
                .reviews
                .push(serde_json::from_value(serde_json::json!({
                    "id":"PM1", "reviewer":"pm", "policy_digest":"fixture-policy",
                    "at":corral_core::clock::now(), "pending_reply":"exact-review-request"
                }))?);
            Ok(())
        })
        .unwrap();
    let source = f.ctx.store.get_assistant_container(&session).unwrap();
    let contract = task.contract_revision().unwrap();
    let wait = "plan review PM1 has no dedicated verifiable native host";
    assert!(continuation::save(&f.ctx, &task.id, &contract, instruction()).is_err());
    let saving = lifecycle::for_task(&f.ctx.store, &f.ctx.store.get(&task.id).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        saving.transfer.unwrap().disposition,
        lifecycle::Disposition::Saving
    );
    assert_eq!(saving.waiting.as_deref(), Some(wait));
    assert!(matches!(
        takeback::apply(&f.ctx, &task.id, &contract, instruction()).unwrap(),
        Returned::Waiting { holders, .. } if holders.iter().any(|s| s == wait)
    ));
    let reopened = corral_core::Store::open(f.ctx.store.root()).unwrap();
    let ctx = crate::Ctx::from_store(reopened);
    recovery::reconcile(&ctx).unwrap();
    let held = ctx.store.get(&task.id).unwrap();
    let journal = lifecycle::for_task(&ctx.store, &held).unwrap().unwrap();
    assert_eq!(
        journal.transfer.as_ref().unwrap().disposition,
        lifecycle::Disposition::TakingBack
    );
    assert!(journal.waiting.as_ref().unwrap().contains(wait));
    assert!(lifecycle::board_owner(&ctx.store, &journal).unwrap());
    assert!(held.dispatch_hold && !held.queued && held.runs.is_empty());
    assert_eq!(
        held.plans, task.plans,
        "no fabricated cancellation, verdict or acknowledgement"
    );
    assert_eq!(held.plan_runs, task.plan_runs);
    assert_eq!(
        ctx.store.get_assistant_container(&session).unwrap(),
        source,
        "no Local publication or source mutation"
    );
    assert!(
        crate::plan_verdict(
            &ctx,
            &task.id,
            &crate::PlanVerdict {
                plan: "P1",
                attempt: Some("PM1"),
                verdict: corral_core::Verdict::Changes,
                notes: "late fixture verdict"
            }
        )
        .is_err()
    );
    assert_eq!(ctx.store.get(&task.id).unwrap().plans, task.plans);
    eprintln!(
        "SHARED_PM_HELD: exact unsupported review wait survives Save/TakeBack/reopen; no cancellation, Local return or late verdict mutation; record/service fixture, not successful PM settlement"
    );
}

#[test]
fn landing_lease_orders_takeback_and_the_recorded_outcome_never_claims_rollback() {
    let (f, session, task) = board();
    let contract = task.contract_revision().unwrap();
    let before = lifecycle::for_task(&f.ctx.store, &task).unwrap().unwrap();
    let owner = f
        .ctx
        .store
        .get_assistant_container(&session)
        .unwrap()
        .workflow
        .unwrap()
        .current;
    let landing = f
        .ctx
        .store
        .try_lock(&format!("autopilot-{}", task.id))
        .unwrap()
        .unwrap();
    assert!(takeback::apply(&f.ctx, &task.id, &contract, instruction()).is_err());
    assert_eq!(
        lifecycle::for_task(&f.ctx.store, &f.ctx.store.get(&task.id).unwrap())
            .unwrap()
            .unwrap(),
        before
    );
    assert!(!f.ctx.store.get(&task.id).unwrap().dispatch_hold);
    // A canonical landing receipt is the trusted input to this service contract.
    // This is a record fixture, NOT evidence of Git advancement, review or shipping.
    let sha = "fixture-recorded-landing";
    f.ctx
        .store
        .update(&task.id, |task| {
            let mut run = ts::run("R1", "fixture-no-process");
            run.outcome = Some(corral_core::RunOutcome::Done);
            run.ended = Some(corral_core::clock::now());
            task.runs.push(run);
            task.status = corral_core::Status::Done;
            task.landed = Some(corral_core::LandingReceipt {
                run: "R1".into(),
                sha: sha.into(),
            });
            Ok(())
        })
        .unwrap();
    drop(landing);
    let result = takeback::apply(&f.ctx, &task.id, &contract, instruction()).unwrap();
    assert!(matches!(result,Returned::AlreadyLanded {sha:actual,..} if actual==sha));
    let after = f.ctx.store.get(&task.id).unwrap();
    assert_eq!(after.status, corral_core::Status::Done);
    assert!(!after.dispatch_hold);
    assert_eq!(
        f.ctx
            .store
            .get_assistant_container(&session)
            .unwrap()
            .workflow
            .unwrap()
            .current,
        owner
    );
    assert_eq!(
        lifecycle::for_task(&f.ctx.store, &after).unwrap().unwrap(),
        before
    );
    eprintln!(
        "LANDING_ORDER: held landing lease prevented take-back mutation; matching canonical record returned exact landed outcome, no Local/rollback claim; record fixture only"
    );
}
