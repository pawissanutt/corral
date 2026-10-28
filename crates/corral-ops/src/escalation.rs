//! Retry choices and their durable provenance.

use anyhow::{Context, Result, bail};
use corral_core::TaskRef as _;
use corral_core::{
    Config, GateRecord, GateResult, ProfileChoice, ProfileSwitch, RetryPlan, ReviewRecord,
    RouteSource, Task, TaskHead, Verdict,
};

fn failed(gate: Option<&GateRecord>, review: Option<&ReviewRecord>, failures: u32) -> u32 {
    let failed = gate.is_some_and(|g| g.result == GateResult::Fail)
        || review.is_some_and(|r| r.verdict == Some(Verdict::Changes));
    failures.max(u32::from(failed))
}

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
            count.saturating_add(failed(run.gate.as_ref(), run.review.as_ref(), run.failures))
        })
}

/// [`failures`] of a head: the streak's earlier runs are read by key, newest first, until the
/// profile changes. Its cost follows the streak, not the task's history.
fn failures_head(head: &TaskHead) -> Result<u32> {
    let Some(current) = head.current_run.as_ref() else {
        return Ok(0);
    };
    let mut count = failed(
        current.gate.as_ref(),
        current.review.as_ref(),
        current.failures,
    );
    let mut back = 1;
    while let Some(run) = head
        .run_head_back(back)?
        .filter(|run| run.agent == current.agent)
    {
        count = count.saturating_add(failed(run.gate.as_ref(), run.review.as_ref(), run.failures));
        back += 1;
    }
    Ok(count)
}

pub(crate) fn escalation(
    config: &Config,
    task: &Task,
    forced: bool,
) -> Result<Option<ProfileSwitch>> {
    escalation_with(config, task, forced, || Ok(failures(task)))
}

fn escalation_with(
    config: &Config,
    task: &Task,
    forced: bool,
    failures: impl FnOnce() -> Result<u32>,
) -> Result<Option<ProfileSwitch>> {
    let run = task.current_run().context("task has no run to escalate")?;
    if !forced
        && (task.profile.is_some() || run.route.as_ref().is_some_and(|source| source.is_manual()))
    {
        return Ok(None);
    }
    let count = failures()?;
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

/// The retry plan of a head: the choice reads head fields and the current run; only the
/// failure streak reads earlier runs, by key.
pub(crate) fn retry_plan_head(
    ctx: &crate::Ctx,
    head: &TaskHead,
    choice: &ProfileChoice,
    forced: bool,
) -> Result<RetryPlan> {
    retry_plan_with(ctx, &head.shell_task()?, choice, forced, || {
        failures_head(head)
    })
}

fn retry_plan_with(
    ctx: &crate::Ctx,
    task: &Task,
    choice: &ProfileChoice,
    forced: bool,
    failures: impl FnOnce() -> Result<u32>,
) -> Result<RetryPlan> {
    let config = &ctx.store.config;
    let run = task.current_run().context("task has no run to retry")?;
    let explicit =
        forced || choice.profile.is_some() || choice.model.is_some() || choice.effort.is_some();
    let current_auto = if !explicit
        && task.profile.is_none()
        && run
            .route
            .as_ref()
            .and_then(RouteSource::auto_selection)
            .is_some()
    {
        Some(crate::routing::route_task(
            ctx,
            task,
            &ProfileChoice::default(),
        )?)
    } else {
        None
    };
    let reclassified = current_auto.as_ref().is_some_and(|route| {
        route.route.auto_selection().map(|receipt| &receipt.id)
            != run
                .route
                .as_ref()
                .and_then(RouteSource::auto_selection)
                .map(|receipt| &receipt.id)
    });
    let selected = choice
        .profile
        .as_ref()
        .or_else(|| (!forced).then_some(task.profile.as_ref()).flatten())
        .or_else(|| {
            current_auto
                .as_ref()
                .filter(|_| reclassified)
                .map(|route| &route.profile)
        });
    let switch = if let Some(profile) = selected {
        (profile != &run.agent).then(|| ProfileSwitch {
            from: run.agent.clone(),
            to: profile.clone(),
            reason: if explicit {
                "explicit retry profile"
            } else {
                "current task selection"
            }
            .into(),
        })
    } else if explicit && !forced {
        None
    } else {
        escalation_with(config, task, forced, failures)?
    };
    // A profile pin bound to an approved candidate keeps its list, not one exact model.
    let candidate_bound = run
        .execution
        .as_ref()
        .and_then(|snapshot| snapshot.candidate.as_ref())
        .is_some_and(|binding| !binding.pinned);
    let keep_previous = choice.profile.is_none()
        && switch.is_none()
        && task.profile.is_none()
        && run.route.as_ref().is_none_or(RouteSource::is_manual)
        && !candidate_bound;
    let profile = switch
        .as_ref()
        .map(|s| s.to.clone())
        .or_else(|| selected.cloned())
        .unwrap_or_else(|| run.agent.clone());
    let source = RouteSource::Retry {
        previous: Box::new(if explicit {
            RouteSource::Override
        } else if task.profile.is_some() {
            RouteSource::Task
        } else if reclassified {
            current_auto.context("missing reclassified route")?.route
        } else {
            run.route.clone().unwrap_or(RouteSource::Fallback)
        }),
        switch: switch.clone(),
    };
    config.check_profile_eligibility(
        &profile,
        corral_core::ProfileRole::Implementation,
        task.project.as_deref(),
        source.requires_automatic_eligibility(),
    )?;
    let overrides = ProfileChoice {
        profile: choice.profile.clone(),
        model: choice
            .model
            .clone()
            .or_else(|| keep_previous.then(|| run.model.clone()).flatten()),
        effort: choice
            .effort
            .clone()
            .or_else(|| keep_previous.then(|| run.effort.clone()).flatten()),
    };
    let resolved = config.resolve_profile(
        &ProfileChoice {
            profile: Some(profile),
            ..overrides.clone()
        },
        None,
        None,
    )?;
    // Without explicit settings, a candidate profile resolves its approved candidate at launch.
    let unpinned = overrides.model.is_none()
        && overrides.effort.is_none()
        && config
            .profiles
            .get(&resolved.profile)
            .and_then(|profile| profile.metadata.as_ref())
            .is_some_and(|metadata| !metadata.candidates.is_empty());
    Ok(RetryPlan {
        source: Some(source),
        overrides: Some(overrides),
        profile: resolved.profile,
        model: resolved.model.filter(|_| !unpinned),
        effort: resolved.effort.filter(|_| !unpinned),
        switch,
    })
}
