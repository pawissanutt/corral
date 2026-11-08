use super::*;
use crate::agent::test_support as ts;
use corral_core::{OperatingActionKind as Kind, TaskRef as _, assistant_workflow::DesignStage};

fn instruction() -> lifecycle::Instruction {
    lifecycle::Instruction::Shell {
        text: "fixture selected current plan source".into(),
        at: corral_core::clock::now(),
    }
}

fn publish(f: &ts::Fx, session: &str, text: &str, goal: Goal) -> Prepared {
    f.ctx
        .store
        .update_assistant_container(session, |c| {
            let w = &mut c.workflow.as_mut().unwrap().current;
            let predecessor = w.current_plan.as_ref().map(|p| p.revision);
            w.current_plan = Some(PlanRevision {
                revision: predecessor.unwrap_or(0) + 1,
                artifact: super::super::publication::artifact(
                    &f.ctx.store,
                    session,
                    &w.id,
                    text.as_bytes(),
                )?,
                stage: DesignStage::Draft,
                assumptions: None,
                execution_scope: None,
                published_at: corral_core::clock::now(),
                supersedes: predecessor,
                scope_changes: String::new(),
            });
            w.state_revision += 1;
            Ok(())
        })
        .unwrap();
    let c = f.ctx.store.get_assistant_container(session).unwrap();
    let w = &c.workflow.as_ref().unwrap().current;
    let expected = Expected {
        workflow: w.id.clone(),
        state_revision: w.state_revision,
        authority_epoch: w.authority_epoch,
        plan_revision: w.current_plan.as_ref().map(|p| p.revision),
    };
    let mut request = tests::request();
    request.goal = goal;
    prepare(&f.ctx.store, session, &c, &expected, request, None).unwrap()
}

fn select(f: &ts::Fx, prepared: &Prepared) -> transfer::Choice {
    transfer::preview(
        &f.ctx,
        prepared.id(),
        "Same scope, current source",
        None,
        false,
    )
    .unwrap()
}

fn returned() -> (ts::Fx, String, corral_core::Task) {
    let mut f = ts::fx();
    let (session, _, _) = tests::planning(&mut f);
    f.ctx.store.save_config().unwrap();
    let prepared = publish(&f, &session, lifecycle_tests::FORMAL, Goal::PlanReviewOnly);
    let choice = select(&f, &prepared);
    let task = transfer::commit(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).unwrap();
    takeback::apply(
        &f.ctx,
        &task.id,
        &task.contract_revision().unwrap(),
        instruction(),
    )
    .unwrap();
    let task = f.ctx.store.get(&task.id).unwrap();
    (f, session, task)
}

#[test]
fn a_new_formal_source_appends_actual_producer_once_without_replacing_prior_plan_history() {
    let (f, session, previous) = returned();
    let newer = lifecycle_tests::FORMAL.replace("Repair cache", "Repair queue");
    let prepared = publish(&f, &session, &newer, Goal::PlanReviewOnly);
    let choice = select(&f, &prepared);
    let result = transfer::commit(&f.ctx, &choice, &choice.digest().unwrap(), instruction());
    assert!(
        result.is_ok(),
        "new exact formal source must append, not reject or relabel old producer: {result:?}"
    );
    let task = result.unwrap();
    assert_eq!(task.id, previous.id);
    assert_eq!(task.plans.len(), 2);
    assert_eq!(task.plan_runs.len(), 2);
    assert_eq!(task.plans[0], previous.plans[0]);
    assert_eq!(task.plan_runs[0], previous.plan_runs[0]);
    assert_eq!(task.plans[1].run.as_deref(), Some("D2"));
    assert_eq!(
        task.plan_runs[1].profile,
        f.ctx.store.get_assistant(&session).unwrap().profile
    );
    assert!(task.plan_runs[1].inbox.settled.is_some());
    assert!(task.plans[1].approval.is_none() && task.plans[1].reviews.is_empty());
    assert_eq!(
        task.plans[1].artifact.sha256,
        corral_core::blob_ref::BlobHash::of(newer.as_bytes()).as_str()
    );
    let replay =
        transfer::commit(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).unwrap();
    assert_eq!(replay.plans, task.plans);
    assert_eq!(replay.plan_runs, task.plan_runs);
    assert!(admission::require(&f.ctx.store, &task, Kind::PlanReview, true).is_ok());
    assert!(task.runs.is_empty() && f.ctx.store.list().unwrap().len() == 1);
}

#[test]
fn a_new_raw_draft_requires_new_formal_planning_without_reusing_predecessor_stopping_semantics() {
    let (f, session, previous) = returned();
    // Invalid as acceptance evidence, deliberately only a historical stopping-state fixture.
    let previous = f
        .ctx
        .store
        .update(&previous.id, |task| {
            task.plan_runs[0].state = corral_core::PlanRunState::Cancelled;
            task.plans[0].approval = Some(corral_core::PlanApproval {
                by: "fixture-only".into(),
                at: corral_core::clock::now(),
                policy_digest: "not-acceptance-evidence".into(),
                authorization: None,
            });
            Ok(())
        })
        .unwrap();
    let prepared = publish(
        &f,
        &session,
        "New unfinished current draft; retain the old plan as history.",
        Goal::PlanContinue,
    );
    let choice = select(&f, &prepared);
    let task = transfer::commit(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).unwrap();
    assert!(
        admission::require(&f.ctx.store, &task, Kind::Plan, true).is_ok(),
        "new raw source must supersede the old plan's stopping state without deleting its history"
    );
    assert!(admission::require(&f.ctx.store, &task, Kind::PlanReview, false).is_err());
    assert!(admission::require(&f.ctx.store, &task, Kind::AcceptPlan, false).is_err());
    assert_eq!(task.plans, previous.plans);
    assert_eq!(task.plan_runs, previous.plan_runs);
    assert!(crate::plan_dispatch::needed(
        &f.ctx,
        &task,
        &crate::DispatchOpts::default()
    ));
    assert!(
        crate::plan_dispatch::replacement_allowed(
            &f.ctx.store,
            &task,
            &crate::DispatchOpts::default()
        )
        .unwrap(),
        "durable current PlanContinue intent must authorize a cancelled planner replacement without a generic force bit"
    );
    let journal = lifecycle::for_task(&f.ctx.store, &task).unwrap().unwrap();
    assert_eq!(
        journal.transfer.unwrap().draft_predecessor.as_deref(),
        Some("P1")
    );
    // The same supersession also ignores an unreviewed predecessor; it is not a
    // generic pending-plan override for ordinary tasks or newly submitted P2.
    let mut unreviewed = task.clone();
    unreviewed.plans[0].approval = None;
    assert!(
        crate::queue::pending_plan_reason(&f.ctx.store, &unreviewed)
            .unwrap()
            .is_none()
    );
    assert!(task.runs.is_empty());
}
