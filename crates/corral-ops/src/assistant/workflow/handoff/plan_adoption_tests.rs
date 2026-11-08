use super::*;
use crate::agent::test_support as ts;
use corral_core::{OperatingActionKind as Kind, TaskRef as _, assistant_workflow::DesignStage};

pub(super) fn instruction() -> lifecycle::Instruction {
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

pub(super) fn board() -> (ts::Fx, String, corral_core::Task) {
    let mut f = ts::fx();
    let (session, _, _) = tests::planning(&mut f);
    f.ctx.store.save_config().unwrap();
    let prepared = publish(&f, &session, lifecycle_tests::FORMAL, Goal::PlanReviewOnly);
    let choice = select(&f, &prepared);
    let task = transfer::commit(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).unwrap();
    (f, session, task)
}

fn returned() -> (ts::Fx, String, corral_core::Task) {
    let (f, session, task) = board();
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
fn a_manual_plan_decision_cannot_mutate_a_saved_transfer_before_its_lifecycle_check() {
    let (f, _, task) = board();
    let saved = continuation::save(
        &f.ctx,
        &task.id,
        &task.contract_revision().unwrap(),
        instruction(),
    )
    .unwrap();
    let result = crate::plan_verdict(
        &f.ctx,
        &task.id,
        &crate::PlanVerdict {
            plan: "P1",
            attempt: None,
            verdict: corral_core::Verdict::Changes,
            notes: "Fixture current owner asks for revisions, without releasing the saved transfer.",
        },
    );
    assert!(
        result.is_err(),
        "manual plan mutation must enforce the saved current-driver fence before recording a decision"
    );
    let current = f.ctx.store.get(&task.id).unwrap();
    assert_eq!(current.plans, saved.plans);
    assert_eq!(current.plan_runs, saved.plan_runs);
    assert!(current.dispatch_hold && current.runs.is_empty());
}

#[test]
fn optional_policy_cannot_turn_an_unapproved_transfer_plan_into_no_plan_requirement() {
    let (f, _, mut task) = returned();
    task.plan_required = Some(false);
    assert!(
        crate::approved_plan(&f.ctx, &task).is_err(),
        "a retained C3 formal plan still needs independent acceptance under optional plan policy"
    );
}

#[test]
fn ordinary_plan_submission_cannot_use_the_unpublished_adoption_domain() {
    let (f, _, task) = board();
    f.ctx
        .store
        .update(&task.id, |task| {
            // Exact lifecycle-boundary fixture: publication is held, not an admitted planner.
            task.plans.clear();
            task.plan_runs.clear();
            task.dispatch_hold = true;
            let mut journal = lifecycle::for_task(&f.ctx.store, task)?.unwrap();
            let transfer = journal.transfer.as_mut().unwrap();
            transfer.ownership_committed = false;
            transfer.adoption_proven = false;
            recovery::save(&f.ctx.store, &journal)
        })
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("plan.md");
    std::fs::write(&path, lifecycle_tests::FORMAL).unwrap();
    assert!(
        crate::plan_submit(&f.ctx, &task.id, &path).is_err(),
        "only the checked internal adopter can submit before ownership publication"
    );
    let task = f.ctx.store.get(&task.id).unwrap();
    assert!(task.plans.is_empty() && task.plan_runs.is_empty() && task.dispatch_hold);
}

#[test]
fn existing_target_findings_change_after_preview_requires_a_new_choice() {
    let (f, session, previous) = returned();
    let prepared = publish(
        &f,
        &session,
        "Current unfinished source draft.",
        Goal::PlanContinue,
    );
    let choice = select(&f, &prepared);
    let changed = f
        .ctx
        .store
        .update(&previous.id, |task| {
            // This is identity-only fixture data, not accepted approval or consent.
            task.plans[0].approval = Some(corral_core::PlanApproval {
                by: "fixture-only".into(),
                at: corral_core::clock::now(),
                policy_digest: "not-acceptance-evidence".into(),
                authorization: None,
            });
            Ok(())
        })
        .unwrap();
    assert_eq!(
        changed.contract_revision().unwrap(),
        previous.contract_revision().unwrap()
    );
    assert!(
        transfer::commit(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).is_err(),
        "target plan/findings changed without changing task contract or run; selected choice must be stale"
    );
    assert_eq!(f.ctx.store.get(&previous.id).unwrap().plans, changed.plans);
    assert_eq!(f.ctx.store.list().unwrap().len(), 1);
    assert!(read_journal(&f.ctx.store, prepared.id()).unwrap().is_none());
    let refreshed = select(&f, &prepared);
    assert_ne!(refreshed.digest().unwrap(), choice.digest().unwrap());
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
fn take_back_retains_the_latest_settled_unsubmitted_planner_draft_without_submitting_it() {
    let (f, session, previous) = returned();
    let prepared = publish(
        &f,
        &session,
        "Source draft before the board's newer work.",
        Goal::PlanContinue,
    );
    let choice = select(&f, &prepared);
    let task = transfer::commit(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).unwrap();
    let output = crate::scratch_reclaim::Attempt::Planner {
        task: &task.id,
        run: "D2",
    }
    .open(&f.ctx)
    .unwrap();
    let latest = "Exact newest unsubmitted planner draft, including unresolved questions.\n";
    std::fs::write(output.join("plan.md"), latest).unwrap();
    let stopped = f
        .ctx
        .store
        .update(&task.id, |task| {
            let mut planner = task.plan_runs.last().unwrap().clone();
            planner.id = "D2".into();
            planner.output_dir = output.clone();
            planner.submitted = None;
            planner.state = corral_core::PlanRunState::Cancelled;
            planner.ended = Some(corral_core::clock::now());
            planner.spec_revision = crate::plan_spec_revision(task)?;
            task.plan_runs.push(planner);
            Ok(())
        })
        .unwrap();
    let row = f
        .ctx
        .store
        .scratch_row(&format!("planner-{}-D2", task.id))
        .unwrap()
        .unwrap();
    assert!(planner_draft::retention(&f.ctx, &row).unwrap().is_some());
    let saved = continuation::save(
        &f.ctx,
        &task.id,
        &stopped.contract_revision().unwrap(),
        instruction(),
    )
    .unwrap();
    assert!(adopt::prompt(&f.ctx, &saved).unwrap().contains(latest));
    assert!(planner_draft::retention(&f.ctx, &row).unwrap().is_none());
    let choice = continuation::preview(&f.ctx, &task.id, Goal::PlanContinue).unwrap();
    std::fs::write(output.join("plan.md"), "Changed after exact preview.").unwrap();
    assert!(
        continuation::start(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).is_err()
    );
    assert!(f.ctx.store.get(&task.id).unwrap().dispatch_hold);
    std::fs::write(output.join("plan.md"), latest).unwrap();
    std::fs::remove_dir_all(&output).unwrap(); // committed context outlives its stopped host's scratch
    assert!(matches!(
        takeback::apply(
            &f.ctx,
            &task.id,
            &stopped.contract_revision().unwrap(),
            instruction()
        )
        .unwrap(),
        Returned::Ready {
            plan_only: true,
            ..
        }
    ));
    let reopened = corral_core::Store::open(f.ctx.store.root()).unwrap();
    let c = reopened.get_assistant_container(&session).unwrap();
    let plan = c
        .workflow
        .as_ref()
        .unwrap()
        .current
        .current_plan
        .as_ref()
        .unwrap();
    assert_eq!(
        super::super::publication::load_artifact(&reopened, &plan.artifact).unwrap(),
        latest.as_bytes(),
        "take-back must preserve the current unsubmitted draft, not rewind to the submitted predecessor"
    );
    assert_eq!(plan.stage, DesignStage::Draft);
    let task = reopened.get(&task.id).unwrap();
    assert_eq!(task.plans, previous.plans);
    assert_eq!(task.plan_runs, stopped.plan_runs);
    assert!(task.runs.is_empty());
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
