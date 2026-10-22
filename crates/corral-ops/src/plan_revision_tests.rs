//! T-251 regression: an owner `changes` verdict resolving plan decisions revises the plan.
use super::*;

const NOTES: &str = "Narrow the storage contract.\nResolved: Choose storage => SQLite";

/// Escalate a plan that declares an owner decision, then enter `mode`.
fn escalated(mode: &str) -> (tempfile::TempDir, Ctx, String) {
    let (tmp, ctx, id, path) = fixture();
    let decided = PLAN.replace("owner_decisions: []", "owner_decisions: [Choose storage]");
    std::fs::write(&path, decided).unwrap();
    ctx.store
        .update(&id, |t| {
            t.plans.clear();
            t.plan_runs[0].state = PlanRunState::Planning;
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
    assert_eq!(task.status, Status::Review);
    assert_eq!(task.plan_runs[0].state, PlanRunState::Escalated);
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
    let kind = corral_core::OperatingActionKind::Plan;
    let auth = crate::operating::load(ctx, id, &run.id, kind, attempt).unwrap();
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
