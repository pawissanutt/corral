//! Review-only uses the normal independent reviewer, not a planner or code pipeline.
//! A verdict is its stopping point; only an already-pending attempt uses normal recovery.
use super::{
    Goal,
    lifecycle::{self, Disposition},
};
use crate::Ctx;
use anyhow::{Context, Result};
use corral_core::{OperatingActionKind, TaskHead, TaskRef};

pub(crate) fn review_only(ctx: &Ctx, task: &TaskHead) -> Result<bool> {
    let Some(journal) = lifecycle::for_task(&ctx.store, task)? else {
        return Ok(false);
    };
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    if transfer.goal != Goal::PlanReviewOnly {
        return Ok(false);
    }
    if transfer.disposition != Disposition::Continuing
        || task.dispatch_hold
        || task.hold.is_some()
        || task.park.is_some()
    {
        return Ok(true);
    }
    let config = ctx.store.config_layers()?.config()?;
    if config.automation.paused || config.dispatch.paused || !config.autopilot_enabled() {
        return Ok(true);
    }
    let target = transfer
        .review_target
        .as_ref()
        .context("plan_review_only: pinned source missing")?;
    let plan = task
        .last_plan_head()?
        .context("plan_review_only: formal plan missing")?;
    if plan.facts.children == target.review_floor {
        crate::operating::require(&ctx.store, task, OperatingActionKind::PlanReview)?;
        let shell = task.shell_task()?;
        if !crate::plan_review::resume_prepared(ctx, &shell)? {
            crate::plan_review::review_auto(ctx, &task.id)?;
        }
        return Ok(true);
    }
    // Recovery of the one pending review still uses plan_pilot's existing disappearance
    // and escalation paths. Terminal findings never enter its author-revision/accept loop.
    Ok(plan
        .current_review
        .as_ref()
        .is_none_or(|r| r.verdict.is_some() || r.cancelled))
}
