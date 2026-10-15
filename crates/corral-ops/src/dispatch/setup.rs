use anyhow::{Context, Result, bail};
use corral_core::{AutoStage, AutopilotState, Run, RunOutcome, Task};
use serde_json::json;

use crate::Ctx;

/// Installs the corral skill for the run's harness (its `flags` map name, else its key) when that harness has
/// none yet under $HOME; logs "skill_installed". A failed install is reported and does not stop the run.
pub(crate) fn install_skill(ctx: &Ctx, id: &str, harness: &str) -> Result<()> {
    let Some(home) = std::env::var_os("HOME") else {
        return Ok(());
    };
    let kind = ctx
        .store
        .config
        .harnesses
        .get(harness)
        .and_then(|h| h.flags.as_deref())
        .unwrap_or(harness);
    match crate::skill::ensure_skill(std::path::Path::new(&home), kind) {
        Ok(Some(path)) => ctx.store.log_event(
            Some(id),
            "skill_installed",
            json!({ "harness": harness, "path": path }),
        ),
        Ok(None) => Ok(()),
        Err(err) => {
            crate::messages::report(
                Some(id),
                "skill",
                format!("installing the corral skill for harness {harness}: {err:#}"),
            );
            Ok(())
        }
    }
}

/// The previous run and its autopilot record when the autopilot requested changes and the run was then
/// superseded (its agent pane was gone): the next run continues that change request.
pub(super) fn carried_changes(task: &Task) -> Option<(Run, AutopilotState)> {
    let prev = task
        .runs
        .last()
        .filter(|r| r.outcome == Some(RunOutcome::Superseded))?;
    let state = prev
        .autopilot
        .clone()
        .filter(|a| a.stage == AutoStage::Changes)?;
    Some((prev.clone(), state))
}

pub(crate) fn check_plan(
    ctx: &Ctx,
    task: &Task,
    route: &corral_core::RouteDecision,
    expected: Option<&corral_core::PlanRecord>,
) -> Result<()> {
    let approved = crate::approved_plan(ctx, task)?;
    if approved.as_ref() != expected {
        bail!(
            "plan approval changed during dispatch; inspect the plan and dispatch explicitly again"
        );
    }
    if let Some(plan) = expected {
        let config = crate::plan::config(ctx)?;
        if config
            .review_policy(task.project.as_deref(), task.review)
            .reviewer
            == corral_core::Reviewer::CrossVendor
        {
            let actual = config.resolve_profile(
                &corral_core::ProfileChoice {
                    profile: Some(route.profile.clone()),
                    model: route.model.clone(),
                    effort: route.effort.clone(),
                },
                None,
                None,
            )?;
            let reviewer = config.cross_vendor_reviewer(
                actual
                    .vendor
                    .as_deref()
                    .context("plan approval requires a known dispatch vendor")?,
            )?;
            if plan.reviews.last().is_none_or(|r| r.reviewer != reviewer) {
                bail!(
                    "stale plan approval: dispatch override requires reviewer {reviewer}; review again"
                );
            }
        }
    }
    Ok(())
}
