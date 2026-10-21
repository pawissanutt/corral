use anyhow::{Context, Result, bail};
use corral_core::{AutoStage, AutopilotState, Run, RunOutcome, Task};
use serde_json::json;

use crate::Ctx;
use crate::skill::SkillAction;

/// Installs or refreshes the role skills for the run's harness (its `flags` map name, else its key) under
/// $HOME; logs "skill_installed" or "skill_refreshed" per written role. A foreign file is left alone with a
/// warning. A failed install is reported and does not stop the run.
pub(crate) fn install_skill(ctx: &Ctx, id: &str, harness: &str) -> Result<()> {
    let Some(home) = std::env::var_os("HOME") else {
        return Ok(());
    };
    install_skill_in(ctx, id, harness, std::path::Path::new(&home))
}

pub(crate) fn install_skill_in(
    ctx: &Ctx,
    id: &str,
    harness: &str,
    home: &std::path::Path,
) -> Result<()> {
    let kind = ctx
        .store
        .config
        .harnesses
        .get(harness)
        .and_then(|h| h.flags.as_deref())
        .unwrap_or(harness);
    match crate::skill::ensure_skills(home, kind) {
        Ok(writes) => {
            for write in writes {
                let event = match write.action {
                    SkillAction::Wrote => "skill_installed",
                    SkillAction::Refreshed => "skill_refreshed",
                    SkillAction::Current => continue,
                    SkillAction::Unmanaged => {
                        crate::messages::report(
                            Some(id),
                            "skill",
                            format!(
                                "{} was not written by corral; left it (run `corral skill install --force` to replace)",
                                write.path.display()
                            ),
                        );
                        "skill_unmanaged"
                    }
                };
                ctx.store.log_event(
                    Some(id),
                    event,
                    json!({ "harness": harness, "role": write.role, "path": write.path }),
                )?;
            }
            Ok(())
        }
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
        let policy = config.task_review_policy(task);
        if plan.run.is_none()
            && policy.require_cross_family
            && policy.reviewer != corral_core::Reviewer::CrossVendor
        {
            let actual = implementation_profile(&config, task, route)?;
            let name = crate::plan::expected_reviewer(&config, task)?;
            if name != "owner" {
                let reviewer = config.resolve_profile(
                    &corral_core::ProfileChoice {
                        profile: Some(if name == "pm" {
                            config.pm.agent.clone()
                        } else {
                            name
                        }),
                        ..Default::default()
                    },
                    None,
                    None,
                )?;
                policy
                    .check_reviewer_family(actual.vendor.as_deref(), reviewer.vendor.as_deref())?;
            }
        }
        if plan.run.is_none()
            && config.task_review_policy(task).reviewer == corral_core::Reviewer::CrossVendor
        {
            let actual = implementation_profile(&config, task, route)?;
            let reviewer = config.cross_vendor_reviewer(
                actual
                    .vendor
                    .as_deref()
                    .context("plan approval requires a known dispatch vendor")?,
            )?;
            if !plan
                .reviews
                .iter()
                .any(|r| r.reviewer == reviewer && r.verdict == Some(corral_core::Verdict::Approve))
            {
                bail!(
                    "stale plan approval: dispatch override requires reviewer {reviewer}; review again"
                );
            }
        }
    }
    Ok(())
}

fn implementation_profile(
    config: &corral_core::Config,
    task: &Task,
    route: &corral_core::RouteDecision,
) -> Result<corral_core::ResolvedProfile> {
    if task.launch.is_some()
        && let Some(snapshot) = task.current_run().and_then(|run| run.execution.as_ref())
    {
        return Ok(snapshot.resolved.clone());
    }
    config.resolve_route(route)
}

#[cfg(test)]
#[path = "setup_skill_tests.rs"]
mod skill_tests;
