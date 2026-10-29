//! Reviewer candidate binding for implementation and plan reviews. A review keeps its
//! reviewer profile, records before-start rejections in its episode, never relaunches a
//! rejected candidate, and refuses to start when every approved candidate is unavailable.

use anyhow::{Context, Result, bail};
use corral_core::{
    AttemptFailure, AvailabilityClass, AvailabilityEpisode, ProfileRole, ResolvedProfile, Run,
    Selection, Task,
};
use jiff::Timestamp;

use super::{Start, before_start, load, metadata, resolve_before_start, save};
use crate::Ctx;

/// One episode per reviewer profile and reviewed run, so another profile never borrows it.
pub(crate) fn review_episode(task: &str, run: &Run, profile: &str) -> String {
    format!("{task}-{}-review-{profile}", run.id)
}

/// The implementer's model family when review independence requires another family.
pub(crate) fn review_exclusion(ctx: &Ctx, task: &Task, run: &Run) -> Option<String> {
    let config = &ctx.store.config;
    let policy = config.task_review_policy(task);
    let cross_vendor =
        crate::review_routing::choice(config, task, None) == corral_core::Reviewer::CrossVendor;
    (cross_vendor || policy.cross_family_required())
        .then(|| crate::reviewer::implementer_vendor(ctx, task, run).ok())
        .flatten()
        .and_then(|vendor| run.family(Some(&vendor)))
        .map(|family| family.to_string())
}

fn unavailable(reason: String, at: Timestamp) -> AttemptFailure {
    AttemptFailure {
        class: AvailabilityClass::Unavailable,
        message: reason,
        retries: 0,
        at,
        recovered_at: None,
    }
}

/// The candidate a code review launches with. The caller rechecks cross-family policy
/// against the returned, actual model.
pub(crate) fn bind_review(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    resolved: ResolvedProfile,
    exclude_provider: Option<&str>,
) -> Result<ResolvedProfile> {
    let config = &ctx.store.config;
    let profile = resolved.profile.clone();
    let Some(metadata) = metadata(config, &profile).filter(|m| m.fallback.is_some()) else {
        return Ok(resolved);
    };
    let id = review_episode(&task.id, run, &profile);
    let now = Timestamp::now();
    let check =
        |candidate: &ResolvedProfile| crate::routing::known_model_unavailability(ctx, candidate);
    let mut episode = match load(ctx, &id)? {
        Some(episode) => episode,
        None => {
            let preferred = &metadata.candidates[0];
            let Some(reason) = check(&config.resolve_candidate(&profile, preferred)?) else {
                return Ok(resolved);
            };
            let mut episode = AvailabilityEpisode::new(
                &id,
                &task.id,
                ProfileRole::ImplementationReview,
                &profile,
                metadata,
                now,
            );
            episode
                .seed(preferred, None, "preferred", now)?
                .failures
                .push(unavailable(reason, now));
            episode
        }
    };
    if let Some(exhausted) = &episode.exhausted {
        bail!(
            "reviewer profile '{profile}' cannot start: {}; attempted: {}",
            exhausted.reason,
            episode.summary()
        );
    }
    loop {
        let current = episode
            .current()
            .context("review episode has no candidate")?
            .clone();
        if current.open_failure().is_none() {
            let candidate = config.execution_candidate(&profile, &current.candidate)?;
            let selected = config.resolve_candidate(&profile, candidate)?;
            match check(&selected) {
                None => {
                    save(ctx, &episode)?;
                    return Ok(selected);
                }
                Some(reason) => {
                    if let Some(attempt) = episode.attempts.last_mut() {
                        attempt.failures.push(unavailable(reason, now));
                    }
                }
            }
        }
        match episode.select(config, metadata, false, exclude_provider) {
            Selection::Next { candidate, .. } => {
                let reason = format!("{} unavailable before review start", current.candidate);
                episode.advance(&candidate, &reason, now)?;
            }
            Selection::Exhausted { reason, .. } | Selection::Blocked { reason } => {
                episode.exhaust(&reason, None, now);
                save(ctx, &episode)?;
                bail!(
                    "reviewer profile '{profile}' cannot start: {reason}; attempted: {}",
                    episode.summary()
                );
            }
        }
    }
}

/// A plan or design review opens on an available approved candidate of its reviewer
/// profile, and independence is rechecked against the model that actually reviews.
pub(crate) fn plan_reviewer(
    ctx: &Ctx,
    task: &Task,
    pm: bool,
    resolved: ResolvedProfile,
) -> Result<ResolvedProfile> {
    let config = &ctx.store.config;
    // The PM reviews in its own session: check the model that session runs, or the one
    // opening it would select, never the preferred model by assumption.
    let selected = if pm {
        match crate::pm::pm_session(ctx)? {
            crate::pm::PmSession::Live(_) => {
                pm_plan_reviewer(ctx, config, task)?;
                return Ok(resolved);
            }
            crate::pm::PmSession::None => {
                let mut check = |candidate: &ResolvedProfile| {
                    crate::routing::known_model_unavailability(ctx, candidate)
                };
                let opening = match metadata(config, &resolved.profile) {
                    Some(metadata) => {
                        before_start(config, &resolved.profile, metadata, None, &mut check)
                    }
                    None => Start::Preferred,
                };
                match opening {
                    Start::Preferred => resolved.clone(),
                    Start::Selected(selected) => selected.0,
                    Start::Exhausted(reason) => bail!("the PM cannot start: {reason}"),
                }
            }
        }
    } else {
        let author = config
            .task_review_policy(task)
            .cross_family_required()
            .then(|| corral_core::plan_policy::plan_author_family(config, task))
            .transpose()?
            .flatten();
        resolve_before_start(
            ctx,
            ProfileRole::PlanReview,
            resolved.clone(),
            author.as_ref().map(|family| family.as_str()),
        )?
    };
    let planner = corral_core::plan_policy::planner(task)?
        .map(corral_core::plan_policy::PlannerProfile::from);
    corral_core::plan_policy::independent_resolved_reviewer(config, task, planner, &selected)?;
    Ok(if pm { resolved } else { selected })
}

/// Cross-family independence for the live PM session's recorded model, checked before a
/// plan is delivered and again when its verdict is accepted. An unknown identity fails closed.
pub(crate) fn pm_plan_reviewer(ctx: &Ctx, config: &corral_core::Config, task: &Task) -> Result<()> {
    if !config.task_review_policy(task).cross_family_required() {
        return Ok(());
    }
    let crate::pm::PmSession::Live(Some(execution)) = crate::pm::pm_session(ctx)? else {
        bail!(
            "the PM session's model is unknown; reopen the PM before it reviews plans under cross-family policy"
        );
    };
    let actual = ResolvedProfile {
        profile: config.pm.agent.clone(),
        harness: execution.harness,
        command: String::new(),
        model: execution.model,
        vendor: execution.vendor,
        effort: None,
        model_args: Vec::new(),
    };
    let planner = corral_core::plan_policy::planner(task)?
        .map(corral_core::plan_policy::PlannerProfile::from);
    corral_core::plan_policy::independent_resolved_reviewer(config, task, planner, &actual)
}
