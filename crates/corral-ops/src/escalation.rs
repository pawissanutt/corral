//! Retry choices and their durable provenance.

use anyhow::{Context, Result, bail};
use corral_core::{Config, GateResult, ProfileChoice, ProfileSwitch, RetryPlan, Task, Verdict};

/// Failures since this profile was selected. Historical runs may predate the counter.
fn failures(task: &Task) -> u32 {
    let Some(current) = task.current_run() else {
        return 0;
    };
    task.runs
        .iter()
        .rev()
        .take_while(|run| run.agent == current.agent)
        .fold(0_u32, |count, run| {
            let failed = run
                .gate
                .as_ref()
                .is_some_and(|g| g.result == GateResult::Fail)
                || run
                    .review
                    .as_ref()
                    .is_some_and(|r| r.verdict == Some(Verdict::Changes));
            count.saturating_add(run.failures.max(u32::from(failed)))
        })
}

pub(crate) fn escalation(
    config: &Config,
    task: &Task,
    forced: bool,
) -> Result<Option<ProfileSwitch>> {
    let run = task.current_run().context("task has no run to escalate")?;
    let count = failures(task);
    if !forced && (config.routing.escalate_after == 0 || count < config.routing.escalate_after) {
        return Ok(None);
    }
    let target = config
        .profiles
        .get(&run.agent)
        .and_then(|p| p.escalate_to.as_ref());
    let Some(target) = target else {
        if forced {
            bail!("profile '{}' has no escalate_to configured", run.agent);
        }
        return Ok(None);
    };
    if target == &run.agent {
        bail!("profile '{}' cannot escalate to itself", run.agent);
    }
    Ok(Some(ProfileSwitch {
        from: run.agent.clone(),
        to: target.clone(),
        reason: if forced {
            "manual changes --escalate".into()
        } else {
            format!(
                "{count} failed rounds on profile {} (escalate_after = {})",
                run.agent, config.routing.escalate_after
            )
        },
    }))
}

pub(crate) fn retry_plan(
    config: &Config,
    task: &Task,
    choice: &ProfileChoice,
    forced: bool,
) -> Result<RetryPlan> {
    let run = task.current_run().context("task has no run to retry")?;
    let switch = if let Some(profile) = &choice.profile {
        (profile != &run.agent).then(|| ProfileSwitch {
            from: run.agent.clone(),
            to: profile.clone(),
            reason: "explicit retry profile".into(),
        })
    } else {
        escalation(config, task, forced)?
    };
    let keep_previous = choice.profile.is_none() && switch.is_none();
    let profile = switch
        .as_ref()
        .map(|s| s.to.clone())
        .or_else(|| choice.profile.clone())
        .unwrap_or_else(|| run.agent.clone());
    let resolved = config.resolve_profile(
        &ProfileChoice {
            profile: Some(profile),
            model: choice
                .model
                .clone()
                .or_else(|| keep_previous.then(|| run.model.clone()).flatten()),
            effort: choice
                .effort
                .clone()
                .or_else(|| keep_previous.then(|| run.effort.clone()).flatten()),
        },
        None,
        None,
    )?;
    Ok(RetryPlan {
        profile: resolved.profile,
        model: resolved.model,
        effort: resolved.effort,
        switch,
    })
}
