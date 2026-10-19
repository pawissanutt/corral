//! Owner-visible automation state uses the same authority checks as the executors.
use anyhow::Result;
use corral_core::{AutoStage, OperatingActionKind, OperatingMode, RunOutcome, Status, Task};

use crate::Ctx;

/// Why automatic review processing cannot take its next step. Collecting existing
/// gate receipts and recording reviewer verdicts remain available in Assist mode.
pub fn automation_blocker(ctx: &Ctx, task: &Task) -> Result<Option<String>> {
    let Some(kind) = next_action(ctx, task)? else {
        return Ok(None);
    };
    Ok(crate::operating::require(&ctx.store, task, kind)
        .err()
        .map(|error| format!("{error:#}")))
}

/// Landing-only consent can consume fresh verification, but cannot refresh stale
/// receipts. The executor and readbacks must choose the same prerequisite.
pub(crate) fn next_action(ctx: &Ctx, task: &Task) -> Result<Option<OperatingActionKind>> {
    if task.status != Status::Review || crate::plan_pilot::handles(task) {
        return Ok(None);
    }
    if task.reviewer.is_none()
        && !ctx
            .store
            .config
            .has_review_policy(task.project.as_deref(), task.review)
    {
        return Ok(None);
    }
    let Some(run) = task
        .current_run()
        .filter(|run| run.outcome == Some(RunOutcome::Done))
    else {
        return Ok(None);
    };
    let kind = match run.autopilot.as_ref().map(|state| state.stage) {
        Some(AutoStage::Reviewed | AutoStage::Landing | AutoStage::Blocked) => {
            let policy = ctx.store.config.task_review_policy(task);
            if (!policy.gate.is_empty() && !crate::verification::gate_passed(ctx, task, run)?)
                || (policy.reviewer != corral_core::Reviewer::None
                    && !crate::verification::review_fresh(ctx, task, run)?)
            {
                OperatingActionKind::Gate
            } else {
                OperatingActionKind::Accept
            }
        }
        Some(AutoStage::Reviewing) => OperatingActionKind::Review,
        Some(AutoStage::Changes) => OperatingActionKind::Repair,
        Some(AutoStage::Escalated | AutoStage::Done) => return Ok(None),
        None | Some(AutoStage::Gating) => OperatingActionKind::Gate,
    };
    Ok(Some(kind))
}

/// A kill switch being enabled does not mean a confirmed policy is effective.
pub fn automation_summary(ctx: &Ctx) -> Result<String> {
    let effective = ctx.store.effective_operating_policy()?;
    let mut reasons = effective.reasons.clone();
    if effective.paused {
        reasons.push("operating policy is paused".into());
    }
    if !ctx.store.config.autopilot_enabled() {
        reasons.push("legacy review automation is explicitly disabled".into());
    }
    let mode = match effective.mode {
        OperatingMode::Assist => "assist",
        OperatingMode::Autopilot => "autopilot",
    };
    if !reasons.is_empty() {
        return Ok(format!(
            "autopilot: blocked ({mode}): {}; existing receipts and verdicts still reconcile",
            reasons.join("; ")
        ));
    }
    Ok(format!(
        "autopilot: scoped ({mode}); grant {}; per-task authority and verification apply",
        effective.authorization.as_deref().unwrap_or("none")
    ))
}
