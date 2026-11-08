use super::plan_adoption_tests::instruction;
use super::*;
use crate::agent::test_support as ts;

fn reserved() -> (ts::Fx, String, Prepared, transfer::Choice) {
    let mut f = ts::fx();
    let (session, c, expected) = tests::planning(&mut f);
    f.ctx.store.save_config().unwrap();
    let prepared = prepare(
        &f.ctx.store,
        &session,
        &c,
        &expected,
        tests::request(),
        None,
    )
    .unwrap();
    f.ctx
        .store
        .settle_assistant_container(&session, |c, _| {
            reserve(&f.ctx.store, &session, c, &prepared)
        })
        .unwrap();
    let choice =
        transfer::preview(&f.ctx, prepared.id(), "Original source choice", None, true).unwrap();
    (f, session, prepared, choice)
}
fn fresh(f: &ts::Fx, session: &str) -> Prepared {
    let c = f.ctx.store.get_assistant_container(session).unwrap();
    let w = &c.workflow.as_ref().unwrap().current;
    let e = Expected {
        workflow: w.id.clone(),
        state_revision: w.state_revision,
        authority_epoch: w.authority_epoch,
        plan_revision: w.current_plan.as_ref().map(|p| p.revision),
    };
    prepare(&f.ctx.store, session, &c, &e, tests::request(), None).unwrap()
}

#[test]
fn reserved_retreat_consumes_old_choice_advances_epoch_and_refresh_never_rearms_it() {
    let (f, session, prepared, old) = reserved();
    let choice = retreat::preview(&f.ctx, prepared.id()).unwrap();
    assert!(choice.card.is_none());
    assert!(matches!(
        retreat::run(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).unwrap(),
        retreat::Outcome::Retreated { .. }
    ));
    let reopened = corral_core::Store::open(f.ctx.store.root()).unwrap();
    let c = reopened.get_assistant_container(&session).unwrap();
    let w = &c.workflow.as_ref().unwrap().current;
    assert_eq!(w.owner, ScopeOwner::Local);
    assert_eq!(w.phase, WorkflowPhase::Planning);
    assert!(
        w.authority_epoch > choice.source.authority_epoch
            && w.state_revision > choice.source.state_revision
    );
    assert!(w.execution.is_none() && reopened.list().unwrap().is_empty());
    let journal = read_journal(&reopened, prepared.id()).unwrap().unwrap();
    assert!(journal.cancelled && journal.source_retained && journal.retreat.unwrap().complete);
    assert!(transfer::commit(&f.ctx, &old, &old.digest().unwrap(), instruction()).is_err());
    let next = fresh(&f, &session);
    assert_ne!(next.id(), prepared.id());
    let selected = transfer::preview(&f.ctx, next.id(), "Fresh exact source", None, true).unwrap();
    let task = transfer::commit(
        &f.ctx,
        &selected,
        &selected.digest().unwrap(),
        instruction(),
    )
    .unwrap();
    let owner = f
        .ctx
        .store
        .get_assistant_container(&session)
        .unwrap()
        .workflow
        .unwrap()
        .current
        .owner;
    assert!(matches!(
        retreat::run(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).unwrap(),
        retreat::Outcome::Retreated { .. }
    ));
    assert_eq!(
        f.ctx
            .store
            .get_assistant_container(&session)
            .unwrap()
            .workflow
            .unwrap()
            .current
            .owner,
        owner
    );
    assert_eq!(f.ctx.store.get(&task.id).unwrap().id, task.id);
}

#[test]
fn partial_card_retreat_preserves_the_card_and_source_then_refresh_reuses_its_history() {
    let (f, session, task) = plan_adoption_tests::board();
    let mut journal = lifecycle::for_task(&f.ctx.store, &task).unwrap().unwrap();
    let expected = match journal.source_boundary.as_ref().unwrap() {
        source_boundary::Proof::Unlaunched { expected, .. } => expected.clone(),
        _ => panic!("no-process fixture"),
    };
    // Exact unit fixture for the atomic pre-ownership boundary: adoption bytes exist,
    // but no Board driver was admitted. No crash process or production epoch rewind.
    journal.transfer.as_mut().unwrap().ownership_committed = false;
    recovery::save(&f.ctx.store, &journal).unwrap();
    f.ctx
        .store
        .update(&task.id, |t| {
            t.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    f.ctx
        .store
        .update_assistant_container(&session, |c| {
            let index = c.workflow.as_mut().unwrap();
            index.authority_epoch = expected.authority_epoch;
            let w = &mut index.current;
            w.owner = ScopeOwner::Local;
            w.phase = WorkflowPhase::HandoffPreparing;
            w.authority_epoch = expected.authority_epoch;
            w.state_revision = expected.state_revision;
            Ok(())
        })
        .unwrap();
    let choice = retreat::preview(&f.ctx, &journal.id).unwrap();
    assert!(choice.published_card);
    assert!(matches!(
        retreat::run(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).unwrap(),
        retreat::Outcome::Retreated { .. }
    ));
    let held = f.ctx.store.get(&task.id).unwrap();
    assert!(held.dispatch_hold && !held.queued && !held.cowork.as_ref().unwrap().owner_confirmed);
    assert_eq!(held.plans, task.plans);
    assert_eq!(held.plan_runs, task.plan_runs);
    let old = lifecycle::for_task(&f.ctx.store, &held).unwrap().unwrap();
    assert_eq!(
        old.transfer.as_ref().unwrap().disposition,
        lifecycle::Disposition::Returned
    );
    assert!(!old.transfer.as_ref().unwrap().ownership_committed);
    let next = fresh(&f, &session);
    let selected =
        transfer::preview(&f.ctx, next.id(), "Continue same retained card", None, true).unwrap();
    assert_eq!(selected.into.as_ref().unwrap().task, task.id);
    let resumed = transfer::commit(
        &f.ctx,
        &selected,
        &selected.digest().unwrap(),
        instruction(),
    )
    .unwrap();
    assert_eq!(resumed.id, task.id);
    assert_eq!(resumed.plans, task.plans);
    assert_eq!(f.ctx.store.list().unwrap().len(), 1);
}

#[test]
fn a_fenced_c1_worker_binding_is_not_positive_retreat_settlement() {
    let f = ts::fx();
    let id = ts::worker_task(&f, "unconfirmed-native");
    ts::bind(
        &f,
        &crate::agent::BindTarget::Worker {
            task: id.clone(),
            run: "R1".into(),
        },
        "unconfirmed-native",
        "fixture-worker",
    );
    f.ctx
        .store
        .update(&id, |task| {
            let run = task.current_run_mut().unwrap();
            run.outcome = Some(corral_core::RunOutcome::Cancelled);
            run.ended = Some(corral_core::clock::now());
            run.agent_host.as_mut().unwrap().binding.health =
                corral_core::agent::HostHealth::Fenced;
            Ok(())
        })
        .unwrap();
    assert!(!retreat::legacy_holders(&f.ctx.store.get(&id).unwrap()).is_empty());
}

#[test]
fn unverified_source_retreat_stays_held_and_restart_never_rolls_it_forward() {
    let (f, session, prepared, old) = reserved();
    f.ctx
        .store
        .update_assistant_container(&session, |c| {
            c.pane = Some("unverified-legacy".into());
            Ok(())
        })
        .unwrap();
    let choice = retreat::preview(&f.ctx, prepared.id()).unwrap();
    assert!(matches!(
        retreat::run(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).unwrap(),
        retreat::Outcome::Waiting { .. }
    ));
    recovery::reconcile(&f.ctx).unwrap();
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    assert_eq!(
        c.workflow.unwrap().current.phase,
        WorkflowPhase::HandoffPreparing
    );
    let j = read_journal(&f.ctx.store, prepared.id()).unwrap().unwrap();
    assert!(j.cancelled && !j.retreat.unwrap().complete && j.waiting.is_some());
    assert!(f.ctx.store.list().unwrap().is_empty());
    assert!(transfer::commit(&f.ctx, &old, &old.digest().unwrap(), instruction()).is_err());
}
