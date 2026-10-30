//! Retries: supersede the current run and dispatch a fresh one, read and written as heads.
use anyhow::{Context, Result, anyhow, bail};
use corral_core::{
    AutoStage, AutopilotState, ProfileChoice, RetryPlan, RunOutcome, Status, TaskHead, TaskHeadMut,
    TaskRef as _,
};
use serde_json::json;

use super::{Admission, DispatchOpts, DispatchOutcome, dispatch_with};
use crate::Ctx;
use crate::cleanup::{Cleanup, cleanup_head};
use crate::integration::RepairKey;

/// Current HEAD of a run's worktree, or None when the worktree is gone or unreadable.
fn worktree_head(worktree: &std::path::Path) -> Option<String> {
    worktree
        .is_dir()
        .then(|| corral_git::rev_sha(worktree, "HEAD").ok())
        .flatten()
}

/// From Review/NeedsMe/Running: marks the current run outcome Superseded (closing its pane when it exists),
/// then dispatches a new run (fresh worktree from base). Explicit profile choices win; otherwise failed
/// rounds can select `escalate_to`. A switch uses the target defaults instead of the old model/effort.
/// The owner redirecting the task resets the autopilot's rounds, but not profile failure history.
pub fn retry(ctx: &Ctx, id: &str, choice: &ProfileChoice) -> Result<DispatchOutcome> {
    retry_with(ctx, id, choice, None, false, None, None, |t| t.rounds = 0)?.full(ctx)
}

/// Guard a retry against an obsolete worker/run identity.
pub fn retry_run(
    ctx: &Ctx,
    id: &str,
    run: &str,
    choice: &ProfileChoice,
) -> Result<DispatchOutcome> {
    retry_with(ctx, id, choice, Some(run), false, None, None, |t| {
        t.rounds = 0
    })?
    .full(ctx)
}

/// Explicitly leaves the live session and starts its configured escalation profile.
pub fn request_changes_escalated(ctx: &Ctx, id: &str, text: &str) -> Result<DispatchOutcome> {
    let task = ctx.store.head(id)?.shell_task()?;
    if !task.awaiting_owner() {
        bail!(
            "{} is {}; changes can only be requested in Review",
            task.id,
            task.status.label()
        );
    }
    retry_with(
        ctx,
        id,
        &ProfileChoice::default(),
        None,
        true,
        None,
        None,
        |t| {
            t.rounds = 0;
            if let Some(run) = t.current_run_mut() {
                run.autopilot = Some(AutopilotState {
                    stage: AutoStage::Changes,
                    reason: Some(text.to_string()),
                    sha: worktree_head(&run.worktree),
                    at: corral_core::clock::now(),
                });
            }
        },
    )?
    .full(ctx)
}

/// The autopilot's fresh retry, when escalation is due or the agent pane is gone. Records round `round`
/// and the request `state` on the superseded run, which `dispatch` appends to the new run's prompt.
pub(crate) fn auto_retry(
    ctx: &Ctx,
    id: &str,
    round: u32,
    state: AutopilotState,
    expected: &RepairKey,
) -> Result<DispatchOutcome<Box<TaskHead>>> {
    let _action = corral_core::work::action(id, corral_core::OperatingActionKind::Repair);
    retry_with(
        ctx,
        id,
        &ProfileChoice::default(),
        None,
        false,
        Some(expected),
        None,
        |t| {
            t.rounds = round;
            if let Some(run) = t.current_run_mut() {
                run.autopilot = Some(state);
            }
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn retry_with(
    ctx: &Ctx,
    id: &str,
    choice: &ProfileChoice,
    expected_run: Option<&str>,
    forced: bool,
    expected: Option<&RepairKey>,
    availability: Option<RetryPlan>,
    mark: impl FnOnce(&mut TaskHeadMut),
) -> Result<DispatchOutcome<Box<TaskHead>>> {
    let automatic = expected.is_some() || availability.is_some();
    let id = corral_core::Store::normalize_id(id)?;
    let owner = ctx
        .store
        .try_lock(&format!("launch-{id}"))?
        .context("dispatch is already in progress for this task")?;
    let head = ctx.store.head(&id)?;
    corral_core::Park::require_unparked_ref(&head)?;
    // The retry choice and checks read head fields and the current run's scalars only.
    let task = head.shell_task()?;
    if let Some(expected) = expected {
        expected.check(&task)?;
        crate::operating::require(&ctx.store, &head, corral_core::OperatingActionKind::Repair)?;
    }
    if task.launch.is_some() {
        bail!(
            "{} has an unfinished launch; recover it before retrying",
            task.id
        );
    }
    if !matches!(
        task.status,
        Status::Review | Status::NeedsMe | Status::Running
    ) {
        bail!(
            "{} is {}; only Running, Needs me and Review tasks can be retried",
            task.id,
            task.status.label()
        );
    }
    let prev = task
        .current_run()
        .cloned()
        .ok_or_else(|| anyhow!("{} has no run to retry", task.id))?;
    if let Some(expected) = expected_run
        && expected != prev.id
    {
        bail!("stale_run: expected {}, received {}", prev.id, expected);
    }
    let plan = match availability.clone() {
        Some(plan) => plan,
        None => crate::escalation::retry_plan_head(ctx, &head, choice, forced)?,
    };
    // Validate the target before closing the working session or superseding its run.
    let decision = crate::routing::route_task(
        ctx,
        &task,
        &ProfileChoice {
            profile: Some(plan.profile.clone()),
            model: plan.model.clone(),
            effort: plan.effort.clone(),
        },
    )?;
    crate::routing::validate_model(ctx, &decision)?;
    let reachable = match &prev.herdr {
        Some(h) => ctx.herdr.pane_get(&h.pane_id)?.is_some(),
        None => false,
    };
    let can_checkpoint = reachable && prev.worktree.is_dir();
    // An availability handoff never depends on the old pane: a missing source tree or
    // checkpoint is not permission to replay work whose outcome is unknown.
    if availability.is_some() {
        anyhow::ensure!(
            prev.worktree.is_dir(),
            "no trustworthy handoff: source worktree {} is missing",
            prev.worktree.display()
        );
        crate::resume::require_checkpoint_head(&head, &prev.id)?;
    } else if can_checkpoint {
        if automatic {
            crate::resume::require_checkpoint_head(&head, &prev.id)?;
        } else {
            // An owner retry may post a checkpoint request through the full task's inbox.
            let full = ctx.store.get(&id)?;
            let current = full.current_run().context("missing_run")?;
            crate::resume::retry_checkpoint(ctx, &full, current)?;
        }
    }
    // Back to Ready so the retried task can be dispatched and does not count against its own slot.
    let (task, ()) = ctx.store.dispatch_update_head(&task.id, |store, t| {
        corral_core::Park::require_unparked_ref(&*t)?;
        if let Some(expected) = expected {
            expected.check(&t.shell_task()?)?;
            crate::operating::require(store, &*t, corral_core::OperatingActionKind::Repair)?;
        }
        if t.head()
            .current_run
            .as_ref()
            .is_none_or(|r| r.id != prev.id)
            || !matches!(t.status, Status::Review | Status::NeedsMe | Status::Running)
        {
            bail!(
                "{}: run {} is no longer current ({}); not retrying",
                t.id,
                prev.id,
                t.status.label()
            );
        }
        if can_checkpoint || availability.is_some() {
            crate::resume::require_checkpoint_head(t.head(), &prev.id)?;
        }
        // Close only the identity validated under this lock. A failed close leaves
        // the current run intact instead of stranding a half-applied retry.
        if reachable && let Some(h) = &prev.herdr {
            ctx.herdr.pane_close(&h.pane_id)?;
        }
        mark(t);
        if let Some(run) = t.current_run_mut() {
            run.outcome = Some(RunOutcome::Superseded);
            run.ended = Some(corral_core::clock::now());
            run.retry = Some(plan.clone());
            if run
                .autopilot
                .as_ref()
                .is_none_or(|a| a.stage != AutoStage::Changes)
            {
                let reason = run
                    .review
                    .as_ref()
                    .filter(|r| r.verdict == Some(corral_core::Verdict::Changes))
                    .and_then(|r| r.notes.clone())
                    .or_else(|| {
                        run.gate
                            .as_ref()
                            .filter(|g| {
                                matches!(
                                    g.result,
                                    corral_core::GateResult::Fail | corral_core::GateResult::Stale
                                )
                            })
                            .map(|g| {
                                format!(
                                    "Gate failed: {}",
                                    g.failed_cmd.as_deref().unwrap_or("unknown command")
                                )
                            })
                    });
                if reason.is_some() {
                    run.autopilot = Some(AutopilotState {
                        stage: AutoStage::Changes,
                        reason,
                        sha: worktree_head(&run.worktree),
                        at: corral_core::clock::now(),
                    });
                }
            }
        }
        t.status = Status::Ready;
        t.queued = true;
        ctx.store.log_event(
            Some(&t.id),
            "superseded",
            json!({ "run": prev.id, "checkpoint_unavailable": !can_checkpoint,
                "source_worktree_missing": !prev.worktree.is_dir() }),
        )?;
        t.dispatch_plan = None;
        Ok(())
    })?;
    cleanup_head(ctx, &task, Cleanup::Round);
    let opts = DispatchOpts {
        plan: false,
        profile: Some(plan.profile),
        model: plan.model,
        effort: plan.effort,
        mode: prev.mode,
        base: Some(prev.base),
        queue_if_blocked: true,
        force_resources: false,
    };
    drop(owner);
    if automatic {
        let inputs = if availability.is_some() {
            json!({"source": "availability", "reason": "approved availability fallback"})
        } else {
            json!({"source": "repair", "reason": "bounded automatic retry"})
        };
        dispatch_with(
            ctx,
            &task.id,
            &opts,
            Admission::Automatic {
                inputs,
                at: corral_core::clock::now(),
            },
        )
    } else {
        dispatch_with(ctx, &task.id, &opts, Admission::Manual)
    }
}
