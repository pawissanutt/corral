use super::*;
use corral_core::{ArtifactKind, HerdrRef};

fn active(ctx: &Ctx, id: &str) -> Task {
    ctx.store
        .update(id, |task| {
            let run = task.plan_runs.last_mut().unwrap();
            run.state = PlanRunState::Planning;
            run.ended = None;
            run.herdr = Some(HerdrRef {
                workspace_id: "w1".into(),
                pane_id: "w1:p1".into(),
                socket: Some(ctx.herdr.socket_path()?),
            });
            task.status = Status::Running;
            Ok(())
        })
        .unwrap()
}

#[test]
fn sync_fails_a_shell_only_planner_without_autopilot() {
    // Given a legacy planner without an exit receipt, still listed as working.
    let (tmp, mut ctx, id, _) = fixture();
    ctx.store.config.review.autopilot = Some(false);
    ctx.store.save_config().unwrap();
    let task = active(&ctx, &id);
    std::fs::write(tmp.path().join("engine/planner-live"), "").unwrap();
    std::fs::write(tmp.path().join("engine/shell-only"), "").unwrap();
    std::fs::write(
        tmp.path().join("engine/output"),
        "panic(main thread): abort() called",
    )
    .unwrap();
    let now = task.plan_runs[0].started + jiff::SignedDuration::from_secs(60);
    // When ordinary sync observes that only the shell remains.
    let changes = crate::sync_at(&ctx, now).unwrap();
    // Then failure evidence and owner attention exist, and no slot remains held.
    let failed = ctx.store.get(&id).unwrap();
    assert_eq!(failed.status, Status::NeedsMe);
    assert_eq!(failed.plan_runs[0].state, PlanRunState::Failed);
    assert_eq!(failed.plan_runs[0].ended, Some(now));
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 0);
    assert!(
        changes
            .iter()
            .any(|c| c.task == id && c.to == Status::NeedsMe)
    );
    let artifact = failed
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Log)
        .unwrap();
    assert_eq!(artifact.run, "D1");
    let log = crate::artifact_read(&ctx, artifact).unwrap();
    assert!(String::from_utf8_lossy(&log).contains("abort()"));
    let events = std::fs::read_to_string(ctx.store.dir().join("events.jsonl")).unwrap();
    assert!(events.lines().any(|line| {
        let event: serde_json::Value = serde_json::from_str(line).unwrap();
        event["kind"] == "needs_me" && event["detail"]["target"] == "plan"
    }));
}

#[test]
fn sync_receipt_fails_immediate_exit_before_startup_grace() {
    // Given a planner whose command has already exited unsuccessfully.
    let (_tmp, ctx, id, _) = fixture();
    let task = active(&ctx, &id);
    std::fs::write(task.plan_runs[0].output_dir.join("exit-0.status"), "134\n").unwrap();
    // When reconciliation runs immediately, even without a missing-pane sample.
    reconcile(&ctx, &task, &Default::default(), task.plan_runs[0].started).unwrap();
    // Then the exit code is durable and its slot is released immediately.
    let failed = ctx.store.get(&id).unwrap();
    assert_eq!(failed.plan_runs[0].state, PlanRunState::Failed);
    assert!(
        failed.plan_runs[0]
            .pending_reply
            .as_deref()
            .unwrap()
            .contains("134")
    );
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 0);
}

#[test]
fn sync_preserves_live_planner_and_unknown_process_observations() {
    // Given a listed planner with a real foreground process.
    let (tmp, ctx, id, _) = fixture();
    let task = active(&ctx, &id);
    std::fs::write(tmp.path().join("engine/planner-live"), "").unwrap();
    let pane = ctx.herdr.pane_get("w1:p1").unwrap().unwrap();
    let panes = [(pane.pane_id.clone(), pane)].into_iter().collect();
    let now = task.plan_runs[0].started + jiff::SignedDuration::from_secs(60);
    // When sync observes a live planner long after startup.
    reconcile(&ctx, &task, &panes, now).unwrap();
    // Then it retains the active session and its slot.
    assert_eq!(
        ctx.store.get(&id).unwrap().plan_runs[0].state,
        PlanRunState::Planning
    );
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 1);
}

#[test]
fn cancel_stale_d1_keeps_p3_pending_owner_approval() {
    // Given three plan revisions, with P3 independently approved but not accepted.
    let (tmp, ctx, id, path) = fixture();
    for _ in 0..2 {
        active(&ctx, &id);
        crate::plan_submit(&ctx, &id, &path).unwrap();
    }
    review(&ctx, &id, Verdict::Approve);
    advance(&ctx, &ctx.store.get(&id).unwrap(), Timestamp::now()).unwrap();
    // Reproduce T-048's erroneous stale D1 alongside an unrelated pending P3.
    let task = active(&ctx, &id);
    let before = task.plans.clone();
    std::fs::write(tmp.path().join("engine/planner-live"), "").unwrap();
    // When the owner cancels the stale planner through the new operation.
    let cancelled = crate::plan_cancel(&ctx, &id).unwrap();
    // Then all revisions/verdicts survive exactly, with no implicit owner approval.
    assert_eq!(cancelled.plans, before);
    assert_eq!(cancelled.plans.last().unwrap().id, "P3");
    assert!(cancelled.plans.last().unwrap().approval.is_none());
    assert_eq!(cancelled.plan_runs[0].state, PlanRunState::Cancelled);
    assert!(cancelled.plan_runs[0].ended.is_some());
    assert_eq!(cancelled.status, Status::NeedsMe);
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 0);
    assert!(!tmp.path().join("engine/planner-live").exists());
    assert_eq!(crate::plan_cancel(&ctx, &id).unwrap().plans, before);
}

#[test]
fn cancellation_refuses_foreign_session_without_closing_pane() {
    // Given a planner owned by another server.
    let (tmp, ctx, id, _) = fixture();
    active(&ctx, &id);
    ctx.store
        .update(&id, |task| {
            task.plan_runs[0].herdr.as_mut().unwrap().socket = Some("/foreign/socket".into());
            Ok(())
        })
        .unwrap();
    std::fs::write(tmp.path().join("engine/planner-live"), "").unwrap();
    // When cancellation selects this server instead.
    assert!(crate::plan_cancel(&ctx, &id).is_err());
    // Then neither the foreign pane nor the run was changed.
    assert!(tmp.path().join("engine/planner-live").exists());
    assert_eq!(
        ctx.store.get(&id).unwrap().plan_runs[0].state,
        PlanRunState::Planning
    );
}

#[test]
fn cancelled_planner_history_does_not_hide_later_coding_slot_or_sync() {
    // Given a cancelled planner retained before a later coding run.
    let (_tmp, ctx, id, _) = fixture();
    active(&ctx, &id);
    crate::plan_cancel(&ctx, &id).unwrap();
    ctx.store
        .update(&id, |task| {
            task.runs.push(serde_json::from_value(json!({
                "id":"R1", "agent":"fixture", "worktree":ctx.store.root(), "branch":"main",
                "base":"main", "base_sha":"fixture", "started":"2026-10-16T00:00:00Z"
            }))?);
            task.status = Status::Running;
            Ok(())
        })
        .unwrap();
    // When admission and sync classify the task.
    let task = ctx.store.get(&id).unwrap();
    let usage = crate::slot_usage(&ctx).unwrap();
    // Then the coding run owns its slot and takes ordinary coding reconciliation.
    assert_eq!(usage.tasks, 1);
    assert!(!handles(&task));
    assert_eq!(task.plan_runs[0].state, PlanRunState::Cancelled);
}
