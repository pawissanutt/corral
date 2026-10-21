//! Host-managed availability fallback within a profile (PRF-09), built on the existing recovery
//! contracts: provider-stall ownership decides, durable episodes record intent before any effect,
//! `retry_with` creates linked continuations from run checkpoints, reviewers get a new review
//! attempt, and blocked or exhausted lists become explicit Waiting or Needs-me states.

use std::path::PathBuf;

use anyhow::{Context, Result};
use corral_core::{
    AttemptFailure, AvailabilityClass, AvailabilityEpisode, CandidateBinding, Config,
    ExecutionCandidate, OperatingActionKind, ProfileChoice, ProfileMetadata, ProfileRole,
    ResolvedProfile, RouteDecision, Run, Selection, SkippedCandidate, Status, Task,
};
use jiff::Timestamp;
use serde_json::json;

use crate::{Change, Ctx};

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

pub(crate) fn review_episode(task: &str, run: &Run) -> String {
    episode_id(task, &run.id, ProfileRole::ImplementationReview)
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

/// The implementer's provider when review independence requires another model family.
pub(crate) fn review_exclusion(ctx: &Ctx, task: &Task, run: &Run) -> Option<String> {
    let config = &ctx.store.config;
    let policy = config.task_review_policy(task);
    let cross_vendor =
        crate::review_routing::choice(config, task, None) == corral_core::Reviewer::CrossVendor;
    (cross_vendor || policy.cross_family_required())
        .then(|| crate::reviewer::implementer_vendor(ctx, task, run).ok())
        .flatten()
}

type Check<'a> = dyn FnMut(&ResolvedProfile) -> Option<String> + 'a;

fn pinned(choice: &ProfileChoice) -> bool {
    choice.model.is_some() || choice.effort.is_some()
}

/// Before start, an unavailable preferred candidate resolves to the next approved one.
fn before_start(
    config: &Config,
    profile: &str,
    metadata: &ProfileMetadata,
    exclude_provider: Option<&str>,
    check: &mut Check<'_>,
) -> Option<(ResolvedProfile, CandidateBinding)> {
    let fallback = metadata.fallback.as_ref().filter(|f| f.automatic)?;
    let preferred = metadata.candidates.first()?;
    let unavailable = check(&config.resolve_candidate(profile, preferred).ok()?)?;
    let mut skipped = vec![SkippedCandidate {
        candidate: preferred.id.clone(),
        reason: unavailable.clone(),
    }];
    for candidate in metadata.candidates.iter().skip(1) {
        if skipped.len() > usize::try_from(fallback.max_switches).unwrap_or(usize::MAX) {
            break;
        }
        let refusal = if exclude_provider == Some(candidate.provider.as_str()) {
            Some("it would violate the cross-family review policy".to_owned())
        } else if let Err(error) = fallback.admits(candidate) {
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
                        return Some((resolved, binding));
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
    None
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
        && metadata(config, profile)
            .is_some_and(|metadata| before_start(config, profile, metadata, None, check).is_some())
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
    if let Some((resolved, binding)) =
        before_start(config, &decision.profile, metadata, None, check)
    {
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
) -> Result<ResolvedProfile> {
    let config = &ctx.store.config;
    let Some(metadata) = metadata(config, &resolved.profile) else {
        return Ok(resolved);
    };
    let mut check =
        |profile: &ResolvedProfile| crate::routing::known_model_unavailability(ctx, profile);
    let Some((fallback, binding)) =
        before_start(config, &resolved.profile, metadata, None, &mut check)
    else {
        return Ok(resolved);
    };
    ctx.store.log_event(
        None,
        "availability_switched",
        json!({"role": role, "profile": resolved.profile, "candidate": binding.candidate,
               "model": fallback.model, "reason": binding.reason, "skipped": binding.skipped}),
    )?;
    Ok(fallback)
}

/// A reviewer launch uses the review episode's selected candidate, or a before-start fallback.
/// The caller rechecks cross-family policy against the returned, actual model.
pub(crate) fn bind_review(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    resolved: ResolvedProfile,
    exclude_provider: Option<&str>,
) -> Result<ResolvedProfile> {
    let config = &ctx.store.config;
    let Some(metadata) = metadata(config, &resolved.profile) else {
        return Ok(resolved);
    };
    if let Some(episode) = load(ctx, &review_episode(&task.id, run))?
        && let Some(next) = episode.pending()
    {
        let candidate = config.execution_candidate(&resolved.profile, &next.candidate)?;
        return config.resolve_candidate(&resolved.profile, candidate);
    }
    let mut check =
        |profile: &ResolvedProfile| crate::routing::known_model_unavailability(ctx, profile);
    Ok(before_start(
        config,
        &resolved.profile,
        metadata,
        exclude_provider,
        &mut check,
    )
    .map_or(resolved, |(fallback, _)| fallback))
}

/// The executor: runs selected continuations after sync releases its pane inventory, so
/// launches take the exclusive ownership claim. Also resumes an interrupted handoff.
pub(crate) fn advance_all(ctx: &Ctx, now: Timestamp) -> Result<Vec<Change>> {
    let mut changes = Vec::new();
    for task in ctx.store.list()? {
        if !crate::session_scope::owns_task(ctx, &task)? {
            continue;
        }
        let Some(run) = task.current_run() else {
            continue;
        };
        if task.status == Status::Review {
            if let Some(child) = run.review_runs.last().filter(|c| c.outcome.is_none())
                && run.review.as_ref().is_some_and(|r| r.verdict.is_none())
                && let Some(reason) = due(ctx, &task.id, child, &review_episode(&task.id, run))?
            {
                changes.extend(crate::reviewer::continue_review(
                    ctx, &task, run, child, &reason, now,
                )?);
            }
        } else if task.status.holds_slot()
            && run.outcome.is_none()
            && let Some(reason) = due(ctx, &task.id, run, &run_episode(&task.id, run))?
        {
            changes.extend(continue_agent(ctx, &task, run, &reason)?);
        }
    }
    Ok(changes)
}

fn due(ctx: &Ctx, task: &str, run: &Run, episode: &str) -> Result<Option<String>> {
    if !crate::provider_stall::continuation_due(ctx, &crate::provider_stall::agent_key(task, run)) {
        return Ok(None);
    }
    let Some(episode) = load(ctx, episode)? else {
        return Ok(None);
    };
    let Some(next) = episode.pending() else {
        return Ok(None);
    };
    let previous = episode.attempts.iter().rev().nth(1);
    Ok(previous
        .is_some_and(|attempt| attempt.run.as_deref() == Some(run.id.as_str()))
        .then(|| next.reason.clone()))
}

fn continue_agent(ctx: &Ctx, task: &Task, run: &Run, reason: &str) -> Result<Option<Change>> {
    let blocked = match crate::operating::require(&ctx.store, task, OperatingActionKind::Dispatch) {
        Err(error) => Some(format!(
            "the approved fallback is ready, but automatic dispatch is not authorized ({error:#}); \
             `corral retry {}` continues with it",
            task.id
        )),
        Ok(()) => match crate::dispatch::availability_retry(ctx, &task.id, &run.id) {
            Ok(_) => None,
            Err(error) => Some(format!("fallback continuation blocked: {error:#}")),
        },
    };
    let current = ctx.store.get(&task.id)?;
    let Some(blocked) = blocked else {
        ctx.store.log_event(
            Some(&task.id),
            "availability_switched",
            json!({"from_run": run.id, "run": current.current_run().map(|r| &r.id),
                   "role": ProfileRole::Implementation, "reason": reason}),
        )?;
        return Ok(Some(Change {
            task: task.id.clone(),
            from: task.status,
            to: current.status,
            note: reason.to_owned(),
            stage: None,
        }));
    };
    let note = format!("{reason}; {blocked}");
    crate::provider_stall::block_continuation(
        ctx,
        &crate::provider_stall::agent_key(&task.id, run),
        &blocked,
    )?;
    ctx.store.log_event(
        Some(&task.id),
        "availability_blocked",
        json!({"run": run.id, "reason": note}),
    )?;
    needs_me(ctx, task, run, &note)
}

fn needs_me(ctx: &Ctx, task: &Task, run: &Run, note: &str) -> Result<Option<Change>> {
    let from = task.status;
    let saved = ctx.store.try_update(&task.id, |t| {
        let Some(current) = t
            .current_run_mut()
            .filter(|r| r.id == run.id && r.outcome.is_none())
        else {
            return Ok(false);
        };
        current.blocked_excerpt = Some(note.to_owned());
        t.status = Status::NeedsMe;
        Ok(true)
    })?;
    if saved.is_none() || from == Status::NeedsMe {
        return Ok(None);
    }
    if let Err(error) = ctx
        .herdr
        .notify(&format!("▲ {} needs you", task.id), note, true)
    {
        crate::messages::report(
            Some(&task.id),
            "notification",
            format!("notification for {} failed: {error:#}", task.id),
        );
    }
    ctx.store.log_event(
        Some(&task.id),
        "needs_me",
        json!({"from": from.as_str(), "note": note}),
    )?;
    Ok(Some(Change {
        task: task.id.clone(),
        from,
        to: Status::NeedsMe,
        note: note.to_owned(),
        stage: None,
    }))
}

#[cfg(test)]
#[path = "availability_tests.rs"]
mod tests;
