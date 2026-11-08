use super::*;
use crate::agent::test_support as ts;
use corral_core::{
    OperatingActionKind as Kind, Status, TaskRef as _, assistant_workflow::DesignStage,
};
use lifecycle::{Disposition, Instruction};

fn instruction() -> Instruction {
    Instruction::Shell {
        text: "explicit selected transfer".into(),
        at: "2026-11-08T00:00:00Z".parse().unwrap(),
    }
}

fn setup(goal: Goal) -> (ts::Fx, String, Prepared) {
    let mut f = ts::fx();
    let (session, container, expected) = tests::planning(&mut f);
    f.ctx.store.save_config().unwrap();
    let mut request = tests::request();
    request.goal = goal;
    let prepared = prepare(&f.ctx.store, &session, &container, &expected, request, None).unwrap();
    (f, session, prepared)
}

fn commit(f: &ts::Fx, prepared: &Prepared, save: bool) -> corral_core::Task {
    let choice =
        transfer::preview(&f.ctx, prepared.id(), "Continue selected scope", None, save).unwrap();
    transfer::commit(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).unwrap()
}

#[test]
fn saved_transfer_is_once_held_after_reopen_and_no_manual_or_automatic_effect_can_resume_it() {
    let (f, session, prepared) = setup(Goal::PlanContinue);
    let task = commit(&f, &prepared, true);
    let again = commit(&f, &prepared, true);
    assert_eq!(task.id, again.id);
    assert_eq!(f.ctx.store.list().unwrap().len(), 1);
    assert!(again.dispatch_hold && !again.queued && again.dispatch_plan.is_none());
    assert!(again.runs.is_empty() && again.plan_runs.is_empty());
    let reopened = corral_core::Store::open(f.ctx.store.root()).unwrap();
    let task = reopened.get(&task.id).unwrap();
    for kind in [
        Kind::Dispatch,
        Kind::Plan,
        Kind::PlanReview,
        Kind::Gate,
        Kind::Review,
        Kind::Repair,
        Kind::Accept,
    ] {
        assert!(
            admission::require(&reopened, &task, kind, false).is_err(),
            "manual {kind:?}"
        );
        assert!(
            admission::require(&reopened, &task, kind, true).is_err(),
            "automatic {kind:?}"
        );
    }
    assert!(!crate::queue::authorized_ready(&reopened, &task));
    let container = reopened.get_assistant_container(&session).unwrap();
    assert_eq!(
        container.workflow.as_ref().unwrap().current.owner,
        ScopeOwner::Board {
            task: task.id.clone()
        }
    );
    assert!(super::super::boundary::require_ordinary(&reopened, &session, &container).is_err());
    assert!(task.plans.is_empty());
    assert!(
        !read_journal(&reopened, prepared.id())
            .unwrap()
            .unwrap()
            .transfer
            .unwrap()
            .notification_pending
    );
}

#[test]
fn explicit_plan_continuation_obeys_scope_and_plan_only_admission_in_assist() {
    let (f, _, prepared) = setup(Goal::PlanContinue);
    let task = commit(&f, &prepared, false);
    assert_eq!(task.status, Status::Ready);
    assert!(crate::operating::require(&f.ctx.store, &task, Kind::Plan).is_ok());
    assert!(crate::plan_dispatch::needed(
        &f.ctx,
        &task,
        &crate::DispatchOpts::default()
    ));
    for kind in [
        Kind::Dispatch,
        Kind::Gate,
        Kind::Review,
        Kind::Repair,
        Kind::Accept,
        Kind::AcceptPlan,
    ] {
        assert!(
            crate::operating::require(&f.ctx.store, &task, kind).is_err(),
            "{kind:?}"
        );
    }
    let changed = f
        .ctx
        .store
        .update(&task.id, |task| {
            task.body.push_str("\nExpanded scope");
            Ok(())
        })
        .unwrap();
    assert!(
        crate::operating::require(&f.ctx.store, &changed, Kind::Plan)
            .unwrap_err()
            .to_string()
            .contains("scope_changed")
    );
}

#[test]
fn save_with_an_unverified_writer_stays_saving_after_restart_and_cannot_reopen_a_host() {
    let (f, _, prepared) = setup(Goal::ImplementationShip);
    let task = commit(&f, &prepared, false);
    assert_eq!(
        task.cowork
            .as_ref()
            .unwrap()
            .completion
            .as_ref()
            .unwrap()
            .intent,
        corral_core::CompletionIntent::Ship
    );
    f.ctx
        .store
        .update(&task.id, |task| {
            task.runs.push(serde_json::from_value(serde_json::json!({
                "id":"R1", "agent":"unknown-legacy", "worktree":f._tmp.path().join("unverified"),
                "branch":"corral/unverified", "base":"main", "base_sha":"a".repeat(40),
                "started":corral_core::clock::now()
            }))?);
            task.status = Status::Running;
            Ok(())
        })
        .unwrap();
    let current = f.ctx.store.get(&task.id).unwrap();
    assert!(
        continuation::save(
            &f.ctx,
            &task.id,
            &current.contract_revision().unwrap(),
            instruction()
        )
        .is_err()
    );
    let reopened = corral_core::Store::open(f.ctx.store.root()).unwrap();
    let held = reopened.get(&task.id).unwrap();
    let journal = lifecycle::for_task(&reopened, &held).unwrap().unwrap();
    assert_eq!(
        journal.transfer.as_ref().unwrap().disposition,
        Disposition::Saving
    );
    assert!(journal.waiting.as_ref().is_some_and(|s| !s.is_empty()));
    assert!(held.dispatch_hold && !held.queued && held.dispatch_plan.is_none());
    assert_eq!(
        held.current_run().unwrap().outcome,
        None,
        "do not invent writer death"
    );
    assert_eq!(
        held.cowork
            .as_ref()
            .unwrap()
            .completion
            .as_ref()
            .unwrap()
            .intent,
        corral_core::CompletionIntent::Verify
    );
    assert!(continuation::preview(&f.ctx, &task.id, Goal::ImplementationShip).is_err());
    assert!(
        crate::agent::open_generation(
            &reopened,
            &crate::agent::BindTarget::Worker {
                task: task.id.clone(),
                run: "R1".into()
            },
            "stale-pane"
        )
        .is_err()
    );
    recovery::reconcile(&f.ctx).unwrap();
    let after = lifecycle::for_task(&reopened, &reopened.get(&task.id).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(after.transfer.unwrap().disposition, Disposition::Saving);
    assert_eq!(reopened.list().unwrap().len(), 1);
}

const FORMAL: &str = "---\ngoal: Repair cache\nnon_goals: []\ndecisions: [keep format]\naffected_paths: [file]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: cache repair, expected: correct value}]\nverification: [unit check]\nowner_decisions: []\n---\n\nExact assistant-produced plan.\n";

#[test]
fn exact_formal_plan_keeps_actual_producer_and_review_only_never_allocates_coding_or_replacement_planner()
 {
    let mut f = ts::fx();
    let (session, _, _) = tests::planning(&mut f);
    f.ctx.store.save_config().unwrap();
    f.ctx
        .store
        .update_assistant_container(&session, |c| {
            let artifact = super::super::publication::artifact(
                &f.ctx.store,
                &session,
                "W1",
                FORMAL.as_bytes(),
            )?;
            c.workflow.as_mut().unwrap().current.current_plan = Some(PlanRevision {
                revision: 1,
                artifact,
                stage: DesignStage::Draft,
                assumptions: None,
                execution_scope: None,
                published_at: corral_core::clock::now(),
                supersedes: None,
                scope_changes: String::new(),
            });
            Ok(())
        })
        .unwrap();
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    let w = &c.workflow.as_ref().unwrap().current;
    let e = Expected {
        workflow: w.id.clone(),
        state_revision: w.state_revision,
        authority_epoch: w.authority_epoch,
        plan_revision: Some(1),
    };
    let mut request = tests::request();
    request.goal = Goal::PlanReviewOnly;
    let prepared = prepare(&f.ctx.store, &session, &c, &e, request, None).unwrap();
    let task = commit(&f, &prepared, false);
    assert!(task.runs.is_empty());
    assert_eq!(task.plan_runs.len(), 1);
    assert!(task.plan_runs[0].herdr.is_none() && !task.plan_runs[0].holds_slot());
    assert_eq!(
        task.plan_runs[0].profile,
        f.ctx.store.get_assistant(&session).unwrap().profile
    );
    assert_eq!(task.plans.len(), 1);
    let plan = &task.plans[0];
    assert_eq!(
        plan.artifact.sha256,
        corral_core::blob_ref::BlobHash::of(FORMAL.as_bytes()).as_str()
    );
    assert!(plan.approval.is_none() && plan.reviews.is_empty());
    assert!(admission::require(&f.ctx.store, &task, Kind::PlanReview, true).is_ok());
    for kind in [
        Kind::Plan,
        Kind::Dispatch,
        Kind::Gate,
        Kind::Review,
        Kind::Accept,
    ] {
        assert!(admission::require(&f.ctx.store, &task, kind, false).is_err());
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("replacement.md");
    std::fs::write(&path, FORMAL.replace("cache", "other")).unwrap();
    assert!(crate::plan_submit(&f.ctx, &task.id, &path).is_err());
    assert_eq!(f.ctx.store.get(&task.id).unwrap().plans, task.plans);
    assert!(
        transfer::preview(
            &f.ctx,
            prepared.id(),
            "Continue selected scope",
            None,
            false
        )
        .is_ok()
    );
    assert!(transfer::preview(&f.ctx, prepared.id(), "changed selection", None, false).is_err());
    assert_eq!(
        read_journal(&f.ctx.store, prepared.id())
            .unwrap()
            .unwrap()
            .transfer
            .unwrap()
            .disposition,
        Disposition::Continuing
    );
    // A later stopped planner does not become the author of the retained exact plan.
    let mut history = task.clone();
    let mut later = task.plan_runs[0].clone();
    later.id = "D2".into();
    later.profile = "not-the-author".into();
    later.state = corral_core::PlanRunState::Cancelled;
    history.plan_runs.push(later);
    let pinned = lifecycle::review_target(&history).unwrap();
    assert_eq!(pinned.producer_run, "D1");
    assert_eq!(pinned.producer_profile, task.plan_runs[0].profile);
    let review_source = crate::plan_review::source_task(&history).unwrap();
    assert_eq!(
        corral_core::plan_policy::planner(&review_source)
            .unwrap()
            .unwrap()
            .profile,
        task.plan_runs[0].profile
    );
    assert!(
        task.unfinished_native_containers().unwrap().is_empty(),
        "an imported producer record did not launch a planner host"
    );
    let changed = f
        .ctx
        .store
        .update(&task.id, |task| {
            task.body.push_str("\nOwner revised the task scope.");
            Ok(())
        })
        .unwrap();
    let planning = continuation::preview(&f.ctx, &task.id, Goal::PlanContinue).expect(
        "stale prior plan is context for renewed planning, not a review-only freshness refusal",
    );
    assert_eq!(
        planning.contract_revision,
        changed.contract_revision().unwrap()
    );
    assert!(continuation::preview(&f.ctx, &task.id, Goal::PlanReviewOnly).is_err());
    let continued = continuation::start(
        &f.ctx,
        &planning,
        &planning.digest().unwrap(),
        instruction(),
    )
    .unwrap();
    assert!(admission::require(&f.ctx.store, &continued, Kind::Plan, false).is_ok());
    assert_eq!(continued.plans, changed.plans);
    assert!(continued.runs.is_empty());
    assert!(matches!(
        takeback::apply(
            &f.ctx,
            &task.id,
            &changed.contract_revision().unwrap(),
            instruction()
        )
        .unwrap(),
        Returned::Ready {
            scratch: None,
            plan_only: true,
            ..
        }
    ));
}

#[test]
fn incomplete_review_only_refuses_without_task_or_reservation_instead_of_replanning() {
    let (f, _, prepared) = setup(Goal::PlanReviewOnly);
    assert!(transfer::preview(&f.ctx, prepared.id(), "Review missing plan", None, false).is_err());
    assert!(f.ctx.store.list().unwrap().is_empty());
    assert!(read_journal(&f.ctx.store, prepared.id()).unwrap().is_none());
}
