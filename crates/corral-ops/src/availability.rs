//! Host-managed availability fallback within a profile (PRF-09), built on the existing recovery
//! contracts: provider-stall ownership decides, durable episodes record intent before any effect,
//! `retry_with` creates linked continuations from run checkpoints, reviewers get a new review
//! attempt, and blocked or exhausted lists become explicit Waiting or Needs-me states.

use std::path::PathBuf;

use anyhow::{Context, Result};
use corral_core::{
    AttemptFailure, AvailabilityClass, AvailabilityEpisode, CandidateBinding, Config,
    ExecutionCandidate, ProfileChoice, ProfileMetadata, ProfileRole, ResolvedProfile,
    RouteDecision, Run, Selection, SkippedCandidate, Task,
};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;

mod executor;
mod review;
pub(crate) use executor::{advance_all, review_continuation_due};
pub(crate) use review::{
    bind_review, plan_reviewer, pm_plan_reviewer, review_episode, review_exclusion,
};

fn role_key(role: ProfileRole) -> &'static str {
    match role {
        ProfileRole::Implementation => "implementation",
        ProfileRole::Planning => "planning",
        ProfileRole::Pm => "pm",
        ProfileRole::PlanReview => "plan-review",
        ProfileRole::ImplementationReview => "review",
    }
}

fn episode_id(task: &str, run: &str, role: ProfileRole) -> String {
    format!("{task}-{run}-{}", role_key(role))
}

/// A continuation inherits its episode, so restarts and new runs never reset the bound.
pub(crate) fn run_episode(task: &str, run: &Run) -> String {
    binding(run)
        .and_then(|binding| binding.episode.clone())
        .unwrap_or_else(|| episode_id(task, &run.id, ProfileRole::Implementation))
}

fn binding(run: &Run) -> Option<&CandidateBinding> {
    run.execution
        .as_ref()
        .and_then(|snapshot| snapshot.candidate.as_ref())
        .filter(|binding| !binding.pinned)
}

fn path(ctx: &Ctx, id: &str) -> PathBuf {
    ctx.store
        .state_dir()
        .join("availability")
        .join(format!("{id}.json"))
}

pub(crate) fn load(ctx: &Ctx, id: &str) -> Result<Option<AvailabilityEpisode>> {
    let path = path(ctx, id);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let episode: AvailabilityEpisode = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing availability episode {}", path.display()))?;
    episode.validate()?;
    Ok(Some(episode))
}

fn save(ctx: &Ctx, episode: &AvailabilityEpisode) -> Result<()> {
    let path = path(ctx, &episode.id);
    let dir = path.parent().context("episode path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(episode)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))
}

fn metadata<'a>(config: &'a Config, profile: &str) -> Option<&'a ProfileMetadata> {
    config
        .profiles
        .get(profile)?
        .metadata
        .as_ref()
        .filter(|metadata| !metadata.candidates.is_empty())
}

/// Profiles with an approved fallback policy, pins included so they get an explicit refusal.
/// Legacy profiles keep the previous transient-only stall behavior.
pub(crate) fn governed(ctx: &Ctx, run: &Run) -> bool {
    metadata(&ctx.store.config, &run.agent).is_some_and(|m| m.fallback.is_some())
}

fn matching<'a>(metadata: &'a ProfileMetadata, run: &Run) -> Option<&'a ExecutionCandidate> {
    metadata.candidates.iter().find(|candidate| {
        run.harness.as_deref() == Some(candidate.harness.as_str())
            && run.model.as_deref() == Some(candidate.model.as_str())
    })
}

pub(crate) struct Failed<'a> {
    pub task: &'a str,
    pub run: &'a Run,
    pub role: ProfileRole,
    pub episode: &'a str,
    pub exclude_provider: Option<&'a str>,
    pub class: AvailabilityClass,
    pub message: &'a str,
    pub since: Timestamp,
    pub retries: u32,
}

pub(crate) enum Assessment {
    /// The next candidate is durably selected; the executor continues after the inventory.
    Switch(String),
    /// Exhausted, but the current candidate has a provider-stated reset.
    Wait { until: Timestamp },
    /// Needs a decision; the detail names the reason and every attempted candidate.
    Stop(String),
}

/// Host policy for a candidate whose retry allowance ended. Never asks the failed model.
pub(crate) fn assess(ctx: &Ctx, failed: &Failed<'_>, now: Timestamp) -> Result<Assessment> {
    let config = &ctx.store.config;
    let profile = &failed.run.agent;
    let Some(metadata) = metadata(config, profile) else {
        return Ok(Assessment::Stop(format!(
            "profile '{profile}' has no approved candidates"
        )));
    };
    let current = match failed.role {
        ProfileRole::Implementation => binding(failed.run)
            .and_then(|binding| config.execution_candidate(profile, &binding.candidate).ok()),
        _ => matching(metadata, failed.run),
    };
    let mut episode = match load(ctx, failed.episode)? {
        Some(episode) => episode,
        None => {
            let Some(current) = current else {
                return Ok(Assessment::Stop(
                    "the run is pinned to an exact model or predates its candidate list, so \
                     automatic substitution is not allowed"
                        .into(),
                ));
            };
            let mut episode = AvailabilityEpisode::new(
                failed.episode,
                failed.task,
                failed.role,
                profile,
                metadata,
                now,
            );
            if let Some(bound) = binding(failed.run) {
                for skipped in &bound.skipped {
                    if let Ok(candidate) = config.execution_candidate(profile, &skipped.candidate) {
                        let reason = format!("skipped before start: {}", skipped.reason);
                        episode.seed(candidate, None, &reason, now)?.failures.push(
                            AttemptFailure {
                                class: AvailabilityClass::Unavailable,
                                message: skipped.reason.clone(),
                                retries: 0,
                                at: failed.run.started,
                                recovered_at: None,
                            },
                        );
                    }
                }
            }
            let reason = binding(failed.run)
                .and_then(|bound| bound.reason.clone())
                .unwrap_or_else(|| "preferred".into());
            episode.seed(current, Some(&failed.run.id), &reason, now)?;
            episode
        }
    };
    if let Some(recorded) = recorded(&episode, failed, now) {
        return Ok(recorded);
    }
    if let Some(actual) = current
        && episode
            .current()
            .is_some_and(|attempt| attempt.candidate != actual.id)
    {
        return Ok(Assessment::Stop(format!(
            "the run used candidate '{}', which is not this episode's current attempt",
            actual.id
        )));
    }
    if let Some(exhausted) = &episode.exhausted {
        return Ok(Assessment::Stop(format!(
            "{}; attempted: {}",
            exhausted.reason,
            episode.summary()
        )));
    }
    let failure = AttemptFailure {
        class: failed.class.clone(),
        message: failed.message.to_owned(),
        retries: failed.retries,
        at: failed.since,
        recovered_at: None,
    };
    if let Err(error) = episode.record_failure(&failed.run.id, failure) {
        return Ok(Assessment::Stop(format!("{error:#}")));
    }
    let from = episode
        .current()
        .map(|attempt| attempt.candidate.clone())
        .unwrap_or_default();
    let selection = episode.select(config, metadata, current.is_none(), failed.exclude_provider);
    let task = Some(failed.task);
    match selection {
        Selection::Next { candidate, skipped } => {
            let approval = metadata
                .fallback
                .as_ref()
                .map_or("", |fallback| fallback.approval.as_str());
            let reason = format!(
                "{from}: {}; continuing {} with {} ({}) under approved fallback policy {approval}",
                failed.class.label(),
                role_key(failed.role),
                candidate.id,
                candidate.model,
            );
            episode.advance(&candidate, &reason, now)?;
            save(ctx, &episode)?;
            ctx.store.log_event(
                task,
                "availability_fallback_selected",
                json!({
                    "episode": episode.id, "run": failed.run.id, "role": failed.role,
                    "profile": profile, "from": from, "to": candidate.id,
                    "class": failed.class, "reason": reason, "skipped": skipped,
                    "attempts": episode.summary(),
                }),
            )?;
            Ok(Assessment::Switch(reason))
        }
        Selection::Exhausted {
            reason,
            waiting_until: Some(until),
        } if until > now => {
            episode.exhaust(&reason, Some(until), now);
            save(ctx, &episode)?;
            let detail = format!(
                "waiting for the provider reset at {until}: {reason}; attempted: {}",
                episode.summary()
            );
            ctx.store.log_event(
                task,
                "availability_waiting",
                json!({"episode": episode.id, "run": failed.run.id, "until": until, "reason": detail}),
            )?;
            if let Err(error) = ctx.herdr.notify(
                &format!("{} waiting for its provider", failed.task),
                &detail,
                false,
            ) {
                crate::messages::report(
                    task,
                    "notification",
                    format!("availability waiting notification failed: {error:#}"),
                );
            }
            Ok(Assessment::Wait { until })
        }
        Selection::Exhausted { reason, .. } | Selection::Blocked { reason } => {
            episode.exhaust(&reason, None, now);
            save(ctx, &episode)?;
            let detail = format!("{reason}; attempted: {}", episode.summary());
            ctx.store.log_event(
                task,
                "availability_exhausted",
                json!({"episode": episode.id, "run": failed.run.id, "reason": detail}),
            )?;
            Ok(Assessment::Stop(detail))
        }
    }
}

/// A redelivered or restarted assessment returns the outcome already recorded for this
/// failure instead of selecting again; a stated reset that has passed ends the wait.
fn recorded(
    episode: &AvailabilityEpisode,
    failed: &Failed<'_>,
    now: Timestamp,
) -> Option<Assessment> {
    let index = episode.attempts.iter().position(|attempt| {
        attempt.run.as_deref() == Some(failed.run.id.as_str())
            && attempt.failures.iter().any(|f| f.at == failed.since)
    })?;
    if let Some(next) = episode.attempts.get(index + 1) {
        return Some(Assessment::Switch(next.reason.clone()));
    }
    let exhausted = episode.exhausted.as_ref()?;
    Some(match exhausted.waiting_until.filter(|until| *until > now) {
        Some(until) => Assessment::Wait { until },
        None => Assessment::Stop(format!(
            "{}; attempted: {}",
            exhausted.reason,
            episode.summary()
        )),
    })
}

/// The stall owner's record of a durable selection for this run, after a restart between
/// the episode write and the stall-state write.
pub(crate) fn selected_for(ctx: &Ctx, episode: &str, run: &str) -> Result<bool> {
    let Some(episode) = load(ctx, episode)? else {
        return Ok(false);
    };
    Ok(episode.pending().is_some()
        && episode
            .attempts
            .iter()
            .rev()
            .nth(1)
            .is_some_and(|attempt| attempt.run.as_deref() == Some(run)))
}

/// The current candidate answered again: an unlaunched switch is dropped, history is kept.
pub(crate) fn recovered(ctx: &Ctx, id: &str, now: Timestamp) -> Result<()> {
    let Some(mut episode) = load(ctx, id)? else {
        return Ok(());
    };
    if episode.recovered(now) {
        save(ctx, &episode)?;
        ctx.store.log_event(
            Some(&episode.task),
            "availability_recovered",
            json!({"episode": episode.id, "attempts": episode.summary()}),
        )?;
    }
    Ok(())
}

pub(crate) type Check<'a> = dyn FnMut(&ResolvedProfile) -> Option<String> + 'a;

pub(crate) enum Start {
    /// The preferred candidate is not known to be unavailable, or no fallback policy applies.
    Preferred,
    Selected(Box<(ResolvedProfile, CandidateBinding)>),
    /// Every approved candidate was rejected; never launch a known unavailable model.
    Exhausted(String),
}

fn pinned(choice: &ProfileChoice) -> bool {
    choice.model.is_some() || choice.effort.is_some()
}

/// Before start, an unavailable preferred candidate resolves to the next approved one.
pub(crate) fn before_start(
    config: &Config,
    profile: &str,
    metadata: &ProfileMetadata,
    exclude_provider: Option<&str>,
    check: &mut Check<'_>,
) -> Start {
    let (Some(policy), Some(preferred)) = (&metadata.fallback, metadata.candidates.first()) else {
        return Start::Preferred;
    };
    let Some(unavailable) = config
        .resolve_candidate(profile, preferred)
        .ok()
        .and_then(|resolved| check(&resolved))
    else {
        return Start::Preferred;
    };
    let mut skipped = vec![SkippedCandidate {
        candidate: preferred.id.clone(),
        reason: unavailable.clone(),
    }];
    let exhausted = |skipped: &[SkippedCandidate], why: &str| {
        let detail = skipped
            .iter()
            .map(|s| format!("{} ({})", s.candidate, s.reason))
            .collect::<Vec<_>>()
            .join("; ");
        Start::Exhausted(format!("{why}; rejected before start: {detail}"))
    };
    if !policy.automatic {
        return exhausted(&skipped, "automatic fallback is off for this profile");
    }
    for candidate in metadata.candidates.iter().skip(1) {
        if skipped.len() > usize::try_from(policy.max_switches).unwrap_or(usize::MAX) {
            break;
        }
        let refusal = if exclude_provider == Some(candidate.provider.as_str()) {
            Some("it would violate the cross-family review policy".to_owned())
        } else if let Err(error) = policy.admits(candidate) {
            Some(format!("{error:#}"))
        } else {
            match config.resolve_candidate(profile, candidate) {
                Err(error) => Some(format!("{error:#}")),
                Ok(resolved) => match check(&resolved) {
                    Some(reason) => Some(reason),
                    None => {
                        let binding = CandidateBinding {
                            candidate: candidate.id.clone(),
                            episode: None,
                            reason: Some(format!(
                                "{} unavailable before start: {unavailable}",
                                preferred.id
                            )),
                            skipped,
                            pinned: false,
                        };
                        return Start::Selected(Box::new((resolved, binding)));
                    }
                },
            }
        };
        if let Some(reason) = refusal {
            skipped.push(SkippedCandidate {
                candidate: candidate.id.clone(),
                reason,
            });
        }
    }
    exhausted(&skipped, "no approved candidate is available")
}

/// Lets routing keep a profile whose preferred model is unavailable but has an approved
/// available alternative, instead of falling through to a different profile.
pub(crate) fn has_alternative(
    config: &Config,
    profile: &str,
    choice: &ProfileChoice,
    check: &mut Check<'_>,
) -> bool {
    !pinned(choice)
        && metadata(config, profile).is_some_and(|metadata| {
            matches!(
                before_start(config, profile, metadata, None, check),
                Start::Selected(..)
            )
        })
}

fn apply(decision: &mut RouteDecision, resolved: &ResolvedProfile, binding: CandidateBinding) {
    decision.harness.clone_from(&resolved.harness);
    decision.model.clone_from(&resolved.model);
    decision.effort.clone_from(&resolved.effort);
    decision.availability = Some(binding);
}

/// Binds a route to its approved candidate: a selected continuation, a before-start fallback,
/// or the preferred candidate. Read-only; explicit model/effort choices are exact pins.
pub(crate) fn bind_route(
    ctx: &Ctx,
    task: &Task,
    role: ProfileRole,
    choice: &ProfileChoice,
    decision: &mut RouteDecision,
    check: &mut Check<'_>,
) -> Result<()> {
    let config = &ctx.store.config;
    let Some(metadata) = metadata(config, &decision.profile) else {
        return Ok(());
    };
    if pinned(choice) || !matches!(role, ProfileRole::Implementation | ProfileRole::Planning) {
        return Ok(());
    }
    if role == ProfileRole::Implementation
        && let Some(previous) = task.current_run()
        && let Some(episode) = load(ctx, &run_episode(&task.id, previous))?
        && episode.profile == decision.profile
        && let Some(next) = episode.current().filter(|attempt| {
            attempt.run.is_none()
                && attempt.failures.is_empty()
                && binding(previous).map(|b| &b.candidate) != Some(&attempt.candidate)
        })
    {
        let candidate = config.execution_candidate(&decision.profile, &next.candidate)?;
        let resolved = config.resolve_candidate(&decision.profile, candidate)?;
        let binding = CandidateBinding {
            candidate: candidate.id.clone(),
            episode: Some(episode.id.clone()),
            reason: Some(next.reason.clone()),
            skipped: Vec::new(),
            pinned: false,
        };
        apply(decision, &resolved, binding);
        return Ok(());
    }
    if let Start::Selected(selected) =
        before_start(config, &decision.profile, metadata, None, check)
    {
        let (resolved, binding) = *selected;
        apply(decision, &resolved, binding);
        return Ok(());
    }
    let preferred = &metadata.candidates[0];
    if decision.harness == preferred.harness
        && decision.model.as_deref() == Some(preferred.model.as_str())
    {
        decision.availability = Some(CandidateBinding {
            candidate: preferred.id.clone(),
            episode: None,
            reason: None,
            skipped: Vec::new(),
            pinned: false,
        });
    }
    Ok(())
}

pub(crate) fn resolve_before_start(
    ctx: &Ctx,
    role: ProfileRole,
    resolved: ResolvedProfile,
    exclude_provider: Option<&str>,
) -> Result<ResolvedProfile> {
    let config = &ctx.store.config;
    let Some(metadata) = metadata(config, &resolved.profile) else {
        return Ok(resolved);
    };
    let mut check =
        |profile: &ResolvedProfile| crate::routing::known_model_unavailability(ctx, profile);
    match before_start(
        config,
        &resolved.profile,
        metadata,
        exclude_provider,
        &mut check,
    ) {
        Start::Preferred => Ok(resolved),
        Start::Exhausted(reason) => anyhow::bail!(
            "{} profile '{}' cannot start: {reason}",
            role_key(role),
            resolved.profile
        ),
        Start::Selected(selected) => {
            let (fallback, binding) = *selected;
            ctx.store.log_event(
                None,
                "availability_switched",
                json!({"role": role, "profile": resolved.profile, "candidate": binding.candidate,
                       "model": fallback.model, "reason": binding.reason, "skipped": binding.skipped}),
            )?;
            Ok(fallback)
        }
    }
}

#[cfg(test)]
#[path = "availability_tests.rs"]
mod tests;
