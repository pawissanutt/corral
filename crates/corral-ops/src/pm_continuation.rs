//! Retain the unique PM container and its native journals, never provider-private memory.
//! The recovery lease serializes quota transfer with every ordinary PM recovery entry.
use anyhow::{Context, Result, ensure};
use corral_core::agent::{AgentPm, AgentRole, AgentRunState, ContainerRef, pm_state};
use corral_core::availability::AvailabilityClass;
use corral_core::connection::BillingMode;
use corral_core::paid_episode::PaidBinding;
use corral_core::records::RecordArea;
use corral_core::{ProfileRole, ResolvedProfile, StoreLock};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::agent::recovery::{PmRecoveryInProgress, PmRecoveryLease};
use crate::availability::{checkpoint, continuation, policy};
use continuation::{ContextSource, IntentPhase, PauseReason, SubjectRevision, Transfer};

const CONTINUATION_PAUSED: RecordArea<u32> = RecordArea::new("pm-continuation-pauses");
const SIGNAL: RecordArea<QuotaSignal> = RecordArea::new("pm-continuation-signals");

#[derive(Deserialize, Serialize, PartialEq, Eq)]
struct QuotaSignal {
    generation: u32,
    turn: u64,
    quota: bool,
}

/// Publish from the native event transaction; never discover quota by scanning PM history.
pub(crate) fn record_signal(ctx: &Ctx, state: &AgentRunState) -> Result<()> {
    let config = &ctx.store.config;
    if state.binding.role != AgentRole::Pm
        || (!policy::profile_configured(config, &config.pm.agent, AgentRole::Pm)
            && state.paid_episode.is_none())
    {
        return Ok(());
    }
    let Some(turn) = state
        .turns
        .iter()
        .filter(|turn| turn.generation == state.binding.generation)
        .max_by_key(|turn| turn.seq)
    else {
        return Ok(());
    };
    let Some(binding) = state
        .current_generation()
        .and_then(|g| g.connection.as_ref())
    else {
        return Ok(());
    };
    if binding.validate().is_err() || binding.connection.billing() != BillingMode::Subscription {
        return Ok(());
    }
    let Some(failure) = turn.provider_failure.as_ref() else {
        return Ok(());
    };
    if failure.connection != binding.connection
        || !matches!(
            failure.availability(),
            Some(AvailabilityClass::QuotaExhausted { .. })
        )
    {
        return Ok(());
    }
    let signal = QuotaSignal {
        generation: turn.generation,
        turn: turn.seq,
        quota: true,
    };
    if SIGNAL.load(&ctx.store, "latest")?.is_some_and(|old| {
        (old.generation, old.turn) > (signal.generation, signal.turn) || old == signal
    }) {
        return Ok(());
    }
    // Only confirmed quota events touch this sticky signal. The supervisor revalidates
    // the canonical latest turn before attaching any financial root.
    SIGNAL.save(&ctx.store, "latest", &signal)
}

fn subject() -> SubjectRevision {
    SubjectRevision {
        task: None,
        spec_revision: None,
        target: None,
    }
}

fn clear_signal(ctx: &Ctx, generation: u32) -> Result<()> {
    if let Some(mut signal) = SIGNAL.load(&ctx.store, "latest")?
        && signal.generation == generation
    {
        signal.quota = false;
        SIGNAL.save(&ctx.store, "latest", &signal)?;
    }
    Ok(())
}

fn pause_continuation(ctx: &Ctx, generation: u32, reason: PauseReason) -> Result<()> {
    continuation::pause(ctx, &ContainerRef::Pm, reason, "PM continuation refused")?;
    CONTINUATION_PAUSED.save(&ctx.store, "generation", &generation)?;
    clear_signal(ctx, generation)
}

/// Test entry without the supervisor's already-held input claim.
#[cfg(test)]
pub(crate) fn advance(ctx: &Ctx, current: &AgentPm, now: Timestamp) -> Result<bool> {
    let Some(input) = ctx.store.try_lock("deliver-pm")? else {
        return Ok(false);
    };
    advance_held(ctx, current, now, &input)
}

/// Session admission precedes financial signals/pauses and every replacement effect.
/// The supervisor retains this claim through recovery settlement and launch bookkeeping;
/// neither stage may reacquire it.
pub(crate) fn advance_held(
    ctx: &Ctx,
    current: &AgentPm,
    now: Timestamp,
    input: &StoreLock,
) -> Result<bool> {
    if !crate::session_control::automation_may_start(ctx, &corral_core::inbox::Recipient::Pm)? {
        return Ok(false);
    }
    let config = &ctx.store.config;
    if !policy::profile_configured(config, &config.pm.agent, AgentRole::Pm)
        && !has_paid_state(&current.state)
    {
        return Ok(false);
    }
    if !SIGNAL
        .load(&ctx.store, "latest")?
        .is_some_and(|signal| signal.quota)
    {
        return Ok(false);
    }
    let lease = match PmRecoveryLease::acquire(ctx) {
        Ok(lease) => lease,
        Err(error) if error.downcast_ref::<PmRecoveryInProgress>().is_some() => return Ok(false),
        Err(error) => return Err(error),
    };
    if lease.pending(ctx) {
        return Ok(false);
    }
    let Some(signal) = SIGNAL
        .load(&ctx.store, "latest")?
        .filter(|signal| signal.quota)
    else {
        return Ok(false);
    };
    let selection = pm_state::Selection::current().key(
        "turns",
        format!("turn:{}:{}", signal.generation, signal.turn),
    );
    let Some(pm) = pm_state::load_selected(&ctx.store, &selection)? else {
        return Ok(false);
    };
    let n = pm.state.binding.generation;
    if n != signal.generation {
        clear_signal(ctx, signal.generation)?;
        return Ok(false);
    }
    if CONTINUATION_PAUSED.load(&ctx.store, "generation")? == Some(n) {
        clear_signal(ctx, n)?;
        return Ok(false);
    }
    let Some(evidence) = policy::quota_evidence(&pm.state, None) else {
        // An open provider-error turn may still settle as quota. Ended ineligible turns
        // (including cancellation/budget exhaustion) and missing turns retire the signal.
        if pm
            .state
            .turns
            .iter()
            .find(|turn| turn.generation == n && turn.seq == signal.turn)
            .is_none_or(|turn| turn.ended.is_some())
        {
            clear_signal(ctx, n)?;
        }
        return Ok(false);
    };
    if evidence.at > now {
        return Ok(false);
    }
    let pm = pm_state::load(&ctx.store)?.context("bound PM disappeared during quota assessment")?;
    if pm.state.binding.generation != n
        || pm
            .state
            .turns
            .iter()
            .filter(|turn| turn.generation == n)
            .max_by_key(|turn| turn.seq)
            .is_none_or(|turn| turn.seq != signal.turn || turn.ended.is_none())
        || policy::quota_evidence(&pm.state, None).is_none_or(|e| e.turn != signal.turn)
    {
        clear_signal(ctx, n)?;
        return Ok(false);
    }
    let refuse = |reason| pause_continuation(ctx, n, reason).map(|()| false);
    let Some(source) = pm
        .state
        .current_generation()
        .and_then(|g| g.connection.as_ref())
    else {
        return refuse(PauseReason::LegacyBinding);
    };
    let profile = ctx.store.config_layers()?.config()?.pm.agent;
    if policy::approval(ctx, &profile, AgentRole::Pm).is_err() {
        return refuse(PauseReason::PolicyNotApproved);
    }
    let target = match policy::target(ctx, &profile, AgentRole::Pm, source) {
        Ok(target) => target,
        Err(_) => return refuse(PauseReason::NoApprovedCandidate),
    };
    // Pre-check before creating the financial root: a blocker leaves native state byte-equal.
    if !crate::pm_rotation::survey(ctx, n, false)?.0.is_empty() {
        return refuse(PauseReason::EnteringEffect);
    }
    if let Err(reason) = checkpoint::quiescent(&pm.state, n) {
        return refuse(reason);
    }
    if super::save_native_context(ctx)
        .and_then(|path| std::fs::read(path).context("reading native PM continuation context"))
        .is_err()
    {
        return refuse(PauseReason::MissingCheckpoint);
    }
    if policy::ensure_root(ctx, &ContainerRef::Pm, &profile, &subject()).is_err() {
        return refuse(PauseReason::PolicyNotApproved);
    }
    if crate::agent::hooks::pm::preflight(ctx, Some(&target.resolved)).is_err() {
        return refuse(PauseReason::UnsupportedHost);
    }
    let decide = || frozen(ctx, &pm, &profile, &target, &evidence.summary);
    match super::successor_with(
        ctx,
        &pm,
        "subscription_quota",
        true,
        Some(&decide),
        Some(&target.resolved),
        Some(input),
    ) {
        Ok(_) => {
            clear_signal(ctx, n)?;
            Ok(true)
        }
        Err(error) => {
            if !crate::operating::refused(&error) {
                pause_continuation(ctx, n, PauseReason::SuccessorAmbiguous)?;
            }
            Err(error.context("launching PM continuation"))
        }
    }
}

/// Recovery calls this decision only after fencing, draining and reconciliation.
fn frozen(
    ctx: &Ctx,
    previous: &AgentPm,
    profile: &str,
    target: &policy::Target,
    reason: &str,
) -> Result<bool> {
    let n = previous.state.binding.generation;
    let refuse = |reason| -> Result<bool> {
        pause_continuation(ctx, n, reason)?;
        Ok(false)
    };
    if !crate::pm_rotation::survey(ctx, n, false)?.0.is_empty() {
        return refuse(PauseReason::EnteringEffect);
    }
    let pm = pm_state::load(&ctx.store)?.context("PM disappeared during continuation")?;
    if pm.state.binding.generation != n {
        return refuse(PauseReason::SubjectChanged);
    }
    let expected = policy::quota_evidence(&previous.state, None);
    let latest = pm
        .state
        .turns
        .iter()
        .filter(|turn| turn.generation == n)
        .max_by_key(|turn| turn.seq);
    if latest.is_none_or(|turn| {
        turn.ended.is_none() || expected.as_ref().is_none_or(|e| e.turn != turn.seq)
    }) || policy::quota_evidence(&pm.state, None).is_none_or(|e| {
        expected
            .as_ref()
            .is_none_or(|expected| e.turn != expected.turn)
    }) {
        return refuse(PauseReason::SubjectChanged);
    }
    let Some(binding) = pm
        .state
        .current_generation()
        .and_then(|g| g.connection.clone())
    else {
        return refuse(PauseReason::LegacyBinding);
    };
    let receipts = match checkpoint::receipt_refs(&pm.state, n) {
        Ok(receipts) => receipts,
        Err(reason) => return refuse(reason),
    };
    let context = match super::save_native_context(ctx)
        .and_then(|path| std::fs::read(path).context("reading frozen native PM context"))
    {
        Ok(context) => context,
        Err(_) => return refuse(PauseReason::MissingCheckpoint),
    };
    let checkpoint = match checkpoint::commit(
        ctx,
        &PaidBinding {
            container: ContainerRef::Pm,
            generation: n,
            binding: binding.clone(),
        },
        &pm.state,
        subject(),
        &context,
        receipts,
    ) {
        Ok(checkpoint) => checkpoint,
        Err(_) => return refuse(PauseReason::CheckpointInvalid),
    };
    let refreshed = match policy::target(ctx, profile, AgentRole::Pm, &binding) {
        Ok(refreshed) => refreshed,
        Err(_) => return refuse(PauseReason::NoApprovedCandidate),
    };
    if refreshed.binding != target.binding || policy::approval(ctx, profile, AgentRole::Pm).is_err()
    {
        return refuse(PauseReason::PolicyNotApproved);
    }
    if let Err(error) = continuation::prepare(
        ctx,
        Transfer {
            source: ContainerRef::Pm,
            successor: ContainerRef::Pm,
            target: refreshed.binding,
            context: ContextSource::Checkpoint {
                digest: checkpoint.digest()?,
            },
            subject: subject(),
            reason: reason.into(),
        },
    ) {
        if crate::operating::refused(&error) {
            return Err(error);
        }
        return refuse(PauseReason::SuccessorAmbiguous);
    }
    Ok(true)
}

/// An API recovery stays on its exact frozen candidate, never today's default route.
pub(crate) fn recovery_profile(ctx: &Ctx, state: &AgentRunState) -> Result<ResolvedProfile> {
    let Some(binding) = state
        .current_generation()
        .and_then(|g| g.connection.as_ref())
        .filter(|binding| binding.connection.api_key())
    else {
        return crate::agent::hooks::pm::profile(ctx);
    };
    let config = ctx.store.config_layers()?.config()?;
    config.check_profile_eligibility(&config.pm.agent, ProfileRole::Pm, None, false)?;
    let candidate = config
        .profiles
        .get(&config.pm.agent)
        .and_then(|p| p.metadata.as_ref())
        .and_then(|m| {
            m.candidates
                .iter()
                .find(|c| Some(c.id.as_str()) == binding.candidate.as_deref())
        })
        .context("paid PM frozen candidate is missing")?;
    let resolved = config.resolve_candidate(&config.pm.agent, candidate)?;
    ensure!(
        resolved.connection_binding()?.as_ref() == Some(binding),
        "paid PM frozen binding changed"
    );
    crate::agent::hooks::pm::preflight(ctx, Some(&resolved))?;
    Ok(resolved)
}

/// Called after recovery's fence, before opening n+1. Unbound/paused successors never retry.
pub(crate) fn regenerate(ctx: &Ctx, state: &AgentRunState) -> Result<()> {
    if !has_paid_state(state) {
        return Ok(());
    }
    if let Err(error) = continuation::regenerate(ctx, &ContainerRef::Pm) {
        if crate::operating::refused(&error) {
            return Err(error);
        }
        continuation::pause(
            ctx,
            &ContainerRef::Pm,
            PauseReason::SuccessorAmbiguous,
            "PM recovery refused",
        )?;
        return Err(error.context("regenerating paid PM"));
    }
    Ok(())
}

fn has_paid_state(state: &AgentRunState) -> bool {
    state.paid_episode.is_some()
        || state
            .current_generation()
            .and_then(|g| g.connection.as_ref())
            .is_some_and(|binding| binding.connection.api_key())
}

pub(crate) fn require_recoverable(ctx: &Ctx, state: &AgentRunState) -> Result<()> {
    if !has_paid_state(state) {
        return Ok(());
    }
    let n = state.binding.generation;
    for generation in [n, n.saturating_add(1)] {
        if let Some(intent) = continuation::intent_for(&ctx.store, &ContainerRef::Pm, generation)?
            && matches!(
                intent.phase,
                IntentPhase::Prepared | IntentPhase::Paused | IntentPhase::Closed
            )
        {
            pause_continuation(ctx, n, PauseReason::SuccessorAmbiguous)?;
            anyhow::bail!("PM continuation successor never bound; owner intervention required");
        }
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
#[path = "pm_continuation_tests.rs"]
mod tests;
