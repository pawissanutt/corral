use super::plan_adoption_tests::{board, instruction};
use super::*;
use crate::agent::test_support as ts;
use corral_core::TaskRef as _;

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
