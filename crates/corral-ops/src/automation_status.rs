//! Owner-visible automation state uses the same authority checks as the executors.
use anyhow::Result;
use corral_core::{AutoStage, OperatingActionKind, OperatingMode, RunOutcome, Status, Task};

use crate::Ctx;

/// Why automatic review processing cannot take its next step. Collecting existing
/// gate receipts and recording reviewer verdicts remain available in Assist mode.
pub fn automation_blocker(ctx: &Ctx, task: &Task) -> Result<Option<String>> {
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
            OperatingActionKind::Accept
        }
        Some(AutoStage::Reviewing) => OperatingActionKind::Review,
        Some(AutoStage::Changes) => OperatingActionKind::Repair,
        Some(AutoStage::Escalated | AutoStage::Done) => return Ok(None),
        None | Some(AutoStage::Gating) => OperatingActionKind::Gate,
    };
    Ok(crate::operating::require(&ctx.store, task, kind)
        .err()
        .map(|error| format!("{error:#}")))
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
