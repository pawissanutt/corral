//! T-251 regression: an owner `changes` verdict resolving plan decisions revises the plan.
use super::*;

const NOTES: &str = "Narrow the storage contract.\nResolved: Choose storage => SQLite";

fn escalated(mode: &str) -> (tempfile::TempDir, Ctx, String) {
    escalated_with(mode, true)
}

/// Escalate a plan that declares an owner decision, then enter `mode`.
/// Without `with_run` the plan is submitted manually, with no planner run.
fn escalated_with(mode: &str, with_run: bool) -> (tempfile::TempDir, Ctx, String) {
    let (tmp, ctx, id, path) = fixture();
    let decided = PLAN.replace("owner_decisions: []", "owner_decisions: [Choose storage]");
    std::fs::write(&path, decided).unwrap();
    ctx.store
        .update(&id, |t| {
            t.plans.clear();
            t.plan_runs[0].state = PlanRunState::Planning;
            if !with_run {
                t.plan_runs.clear();
                t.status = Status::Ready;
            }
            Ok(())
        })
        .unwrap();
    let plan = crate::plan_submit(&ctx, &id, &path).unwrap();
    // Owner decisions deny automatic review, so the owner launches it.
    let attempt = crate::plan_review(&ctx, &id, None).unwrap();
    crate::plan_verdict(
        &ctx,
        &id,
        &crate::PlanVerdict {
            plan: &plan.id,
            attempt: Some(&attempt.id),
            verdict: Verdict::Escalate,
            notes: "Owner must choose storage",
        },
    )
    .unwrap();
    advance(&ctx, &ctx.store.get(&id).unwrap(), Timestamp::now()).unwrap();
    match mode {
        "autopilot" => {}
        "paused" => {
            ctx.store.pause_operating_policy(true).unwrap();
        }
        "assist" => {
            ctx.store
                .edit_config(|doc| doc.set_str("automation.mode", "assist"))
                .unwrap();
        }
        _ => unreachable!(),
    }
    let mut ctx = ctx;
    ctx.store.reload_config_if_changed().unwrap();
    let task = ctx.store.get(&id).unwrap();
    let expected = if with_run {
        Status::Review
    } else {
        Status::NeedsMe
    };
    assert_eq!(task.status, expected);
    if with_run {
        assert_eq!(task.plan_runs[0].state, PlanRunState::Escalated);
    } else {
        assert!(task.plan_runs.is_empty());
    }
    assert_eq!(
        ctx.store.effective_operating_policy().unwrap().mode,
        if mode == "assist" {
            corral_core::OperatingMode::Assist
        } else {
            corral_core::OperatingMode::Autopilot
        }
    );
    let _ = std::fs::remove_file(tmp.path().join("engine/launch-command"));
    (tmp, ctx, id)
}

fn owner_changes(ctx: &Ctx, id: &str) {
    let plan = ctx.store.get(id).unwrap().plans.last().unwrap().id.clone();
    let review = crate::plan_verdict(
        ctx,
        id,
        &crate::PlanVerdict {
            plan: &plan,
            attempt: None,
            verdict: Verdict::Changes,
            notes: NOTES,
        },
    )
    .unwrap();
    assert_eq!(review.verdict, Some(Verdict::Changes));
}

fn assert_revising(tmp: &tempfile::TempDir, ctx: &Ctx, id: &str, attempt: &str, owner: bool) {
    let task = ctx.store.get(id).unwrap();
    let run = &task.plan_runs[0];
    assert_eq!(task.status, Status::Running);
    assert_eq!(run.state, PlanRunState::Changes);
    assert_eq!(run.rounds, 1);
    assert!(run.pending_reply.is_none());
    assert!(tmp.path().join("engine/launch-command").exists());
    let prompt = std::fs::read_to_string(run.output_dir.join("prompt.md")).unwrap();
    assert!(
        prompt.contains("Resolved: Choose storage => SQLite"),
        "{prompt}"
    );
    let claim: serde_json::Value =
        serde_json::from_slice(&std::fs::read(run.output_dir.join("launch.json")).unwrap())
            .unwrap();
    let saved = claim["attempt"].as_str().unwrap();
    assert!(saved.starts_with(attempt), "{saved}");
    let kind = corral_core::OperatingActionKind::Plan;
    let auth = crate::operating::load(ctx, id, &run.id, kind, saved).unwrap();
    assert_eq!(crate::operating::is_automatic(&auth), !owner);
}

#[test]
fn owner_changes_verdict_starts_revision_in_every_operating_mode() {
    for mode in ["autopilot", "paused", "assist"] {
        // Given the T-251 sequence: a decision plan escalated to the owner.
        let (tmp, ctx, id) = escalated(mode);
        // When the owner requests changes with Resolved lines.
        owner_changes(&ctx, &id);
        // Then the owner's verdict alone starts the revision planner with the notes.
        assert_revising(&tmp, &ctx, &id, "owner-revision-1", true);
    }
}

#[test]
fn owner_revision_queues_while_dispatch_is_paused_and_resumes_after() {
    // Given an escalated decision plan while dispatch is paused.
    let (tmp, mut ctx, id) = escalated("autopilot");
    ctx.store.config.dispatch.paused = true;
    ctx.store.save_config().unwrap();
    // When the owner requests changes.
    owner_changes(&ctx, &id);
    // Then the revision is queued with its notes, not launched or lost.
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::Review);
    assert_eq!(task.plan_runs[0].pending_reply.as_deref(), Some(NOTES));
    assert!(!tmp.path().join("engine/launch-command").exists());
    // And it starts on the next sync once dispatch resumes.
    ctx.store.config.dispatch.paused = false;
    ctx.store.save_config().unwrap();
    advance(&ctx, &task, Timestamp::now()).unwrap();
    assert_revising(&tmp, &ctx, &id, "owner-revision-1", true);
}

#[test]
fn autopilot_revision_counts_owner_changes_resolutions() {
    use corral_core::OperatingActionKind::{AcceptPlan, Dispatch, Plan, PlanReview};
    // Given an owner changes verdict whose immediate revision was held.
    let (tmp, ctx, id) = escalated("autopilot");
    ctx.store
        .update(&id, |t| {
            t.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    owner_changes(&ctx, &id);
    assert!(!tmp.path().join("engine/launch-command").exists());
    // When the hold clears.
    ctx.store
        .update(&id, |t| {
            t.dispatch_hold = false;
            Ok(())
        })
        .unwrap();
    // Then its resolutions authorize only the automatic revision.
    let effective = ctx.store.effective_operating_policy().unwrap();
    assert!(effective.allows(&id, Plan), "{:?}", effective.tasks[&id]);
    for kind in [Dispatch, PlanReview, AcceptPlan] {
        assert!(!effective.allows(&id, kind), "{kind:?}");
    }
    // And the plan pilot reconciles, its own revision path starts instead of refusing.
    advance(&ctx, &ctx.store.get(&id).unwrap(), Timestamp::now()).unwrap();
    assert_revising(&tmp, &ctx, &id, "revision-1", false);
}

#[test]
fn dispatch_plan_revises_a_review_task_with_a_changes_verdict() {
    // Given a Review task whose owner changes verdict did not start a planner.
    let (tmp, ctx, id) = escalated("assist");
    ctx.store
        .update(&id, |t| {
            t.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    owner_changes(&ctx, &id);
    ctx.store
        .update(&id, |t| {
            t.dispatch_hold = false;
            t.status = Status::Review;
            Ok(())
        })
        .unwrap();
    // When the owner explicitly dispatches a replacement plan.
    let opts = crate::DispatchOpts {
        plan: true,
        ..Default::default()
    };
    let outcome = crate::dispatch::dispatch(&ctx, &id, &opts).unwrap();
    // Then the revision planner starts with the verdict's notes.
    assert!(matches!(outcome, crate::DispatchOutcome::Started { .. }));
    assert_revising(&tmp, &ctx, &id, "owner-revision-1", true);
}

/// A fresh owner design run, as opposed to a revision of an existing planner.
fn assert_fresh_planner(tmp: &tempfile::TempDir, ctx: &Ctx, id: &str, run_id: &str) {
    let task = ctx.store.get(id).unwrap();
    let run = task.plan_runs.last().unwrap();
    assert_eq!(run.id, run_id);
    assert_eq!(task.status, Status::Running);
    assert_eq!(run.state, PlanRunState::Planning);
    assert_eq!(run.spec_revision, crate::plan_spec_revision(&task).unwrap());
    assert!(run.pending_reply.is_none());
    assert!(tmp.path().join("engine/launch-command").exists());
    let prompt = std::fs::read_to_string(run.output_dir.join("prompt.md")).unwrap();
    assert!(prompt.contains(NOTES), "{prompt}");
    let mut action =
        crate::operating::identity(id, run_id, corral_core::OperatingActionKind::Plan, "launch");
    action.round = Some(0);
    let auth = crate::operating::load_start(ctx, &action).unwrap().unwrap();
    assert!(!crate::operating::is_automatic(&auth));
}

#[test]
fn owner_changes_on_a_manually_submitted_plan_starts_a_planner_with_its_notes() {
    for mode in ["autopilot", "paused", "assist"] {
        // Given an escalated plan that was submitted without any planner run.
        let (tmp, ctx, id) = escalated_with(mode, false);
        // When the owner requests changes.
        owner_changes(&ctx, &id);
        // Then a fresh owner-authorized design run starts carrying the notes.
        assert_fresh_planner(&tmp, &ctx, &id, "D1");
    }
}

#[test]
fn explicit_replacement_supersedes_a_queued_revision_after_the_spec_changed() {
    // Given an owner revision queued while dispatch is paused.
    let (tmp, mut ctx, id) = escalated("assist");
    ctx.store.config.dispatch.paused = true;
    ctx.store.save_config().unwrap();
    owner_changes(&ctx, &id);
    assert_eq!(ctx.store.get(&id).unwrap().status, Status::Review);
    // When the task contract changes and dispatch resumes.
    ctx.store
        .update(&id, |t| {
            t.body.push_str("Also support export.\n");
            Ok(())
        })
        .unwrap();
    ctx.store.config.dispatch.paused = false;
    ctx.store.save_config().unwrap();
    // Then the stale queued action is refused without stranding the task.
    let task = ctx.store.get(&id).unwrap();
    assert!(advance(&ctx, &task, Timestamp::now()).is_err());
    assert!(!tmp.path().join("engine/launch-command").exists());
    assert_eq!(ctx.store.get(&id).unwrap().status, Status::Review);
    // And an explicit replacement starts a current-spec planner with the notes.
    let opts = crate::DispatchOpts {
        plan: true,
        ..Default::default()
    };
    let outcome = crate::dispatch::dispatch(&ctx, &id, &opts).unwrap();
    assert!(matches!(outcome, crate::DispatchOutcome::Started { .. }));
    assert_fresh_planner(&tmp, &ctx, &id, "D2");
}

#[test]
fn new_owner_verdict_supersedes_a_queued_revision_bound_to_old_policy() {
    // Given an owner revision queued while dispatch is paused.
    let (tmp, mut ctx, id) = escalated("assist");
    ctx.store.config.dispatch.paused = true;
    ctx.store.save_config().unwrap();
    owner_changes(&ctx, &id);
    // When review policy changes, dispatch resumes and the owner records new changes.
    ctx.store.config.review.max_rounds = Some(2);
    ctx.store.config.dispatch.paused = false;
    ctx.store.save_config().unwrap();
    owner_changes(&ctx, &id);
    // Then the new verdict launches with fresh, current owner consent.
    assert_revising(&tmp, &ctx, &id, "owner-revision-1-", true);
}

#[test]
fn queued_owner_replacement_for_a_run_less_plan_survives_autopilot_sync() {
    for entry in ["verdict", "explicit"] {
        for mode in ["autopilot", "paused"] {
            // Given a manually submitted, escalated plan under a valid Autopilot grant.
            let (tmp, mut ctx, id) = escalated_with("autopilot", false);
            ctx.store.config.dispatch.paused = true;
            ctx.store.save_config().unwrap();
            if mode == "paused" {
                ctx.store.pause_operating_policy(true).unwrap();
            }
            // When the owner's blocked request is queued by the verdict or explicitly.
            owner_changes(&ctx, &id);
            if entry == "explicit" {
                let opts = crate::DispatchOpts {
                    plan: true,
                    queue_if_blocked: true,
                    ..Default::default()
                };
                let outcome = crate::dispatch::dispatch(&ctx, &id, &opts).unwrap();
                assert!(matches!(outcome, crate::DispatchOutcome::Queued { .. }));
            }
            let task = ctx.store.get(&id).unwrap();
            assert!(task.queued && task.plan_runs.is_empty(), "{entry} {mode}");
            assert_eq!(task.status, Status::Ready);
            // And dispatch (and automation) resume before ordinary sync.
            if mode == "paused" {
                ctx.store.pause_operating_policy(false).unwrap();
            }
            ctx.store.config.dispatch.paused = false;
            ctx.store.save_config().unwrap();
            let allows = ctx.store.effective_operating_policy().unwrap();
            assert!(allows.allows(&id, corral_core::OperatingActionKind::Plan));
            crate::sync_at(&ctx, Timestamp::now()).unwrap();
            if mode == "autopilot" {
                // Then the queued owner planner starts with the notes instead of escalating.
                assert_fresh_planner(&tmp, &ctx, &id, "D1");
                continue;
            }
            // Then a start approved under the paused policy revision is refused, not
            // escalated or stranded, and a fresh owner request starts the planner.
            let task = ctx.store.get(&id).unwrap();
            assert_eq!(task.status, Status::Ready, "{entry}");
            assert!(task.plan_runs.iter().all(|r| !r.holds_slot()));
            assert!(!tmp.path().join("engine/launch-command").exists());
            let opts = crate::DispatchOpts {
                plan: true,
                ..Default::default()
            };
            crate::dispatch::dispatch(&ctx, &id, &opts).unwrap();
            assert_fresh_planner(&tmp, &ctx, &id, "D2");
        }
    }
}
