//! Retain the unique PM container and its native journals, never provider-private memory.
//! The recovery lease serializes quota transfer with every ordinary PM recovery entry.
use anyhow::{Context, Result, ensure};
use corral_core::agent::{AgentPm, AgentRole, AgentRunState, ContainerRef, pm_state};
use corral_core::paid_episode::PaidBinding;
use corral_core::records::RecordArea;
use corral_core::{ProfileRole, ResolvedProfile};
use jiff::Timestamp;

use crate::Ctx;
use crate::agent::recovery::{PmRecoveryInProgress, PmRecoveryLease};
use crate::availability::{checkpoint, continuation, policy};
use continuation::{ContextSource, IntentPhase, PauseReason, SubjectRevision, Transfer};

const PAUSED: RecordArea<u32> = RecordArea::new("pm-continuation-pauses");

fn subject() -> SubjectRevision {
    SubjectRevision {
        task: None,
        spec_revision: None,
        target: None,
    }
}

fn pause(ctx: &Ctx, generation: u32, reason: PauseReason) -> Result<()> {
    continuation::pause(ctx, &ContainerRef::Pm, reason, "PM continuation refused")?;
    PAUSED.save(&ctx.store, "generation", &generation)
}

/// Busy recovery is owned by the winner; the supervisor must not open a second PM.
pub(crate) fn advance(ctx: &Ctx, now: Timestamp) -> Result<()> {
    let lease = match PmRecoveryLease::acquire(ctx) {
        Ok(lease) => lease,
        Err(error) if error.downcast_ref::<PmRecoveryInProgress>().is_some() => return Ok(()),
        Err(error) => return Err(error),
    };
    if lease.pending(ctx) {
        return Ok(());
    }
    let Some(pm) =
        pm_state::load_selected(&ctx.store, &pm_state::Selection::current().family("turns"))?
    else {
        return Ok(());
    };
    let n = pm.state.binding.generation;
    if PAUSED.load(&ctx.store, "generation")? == Some(n) {
        return Ok(());
    }
    let Some(evidence) = policy::quota_evidence(&pm.state, None) else {
        return Ok(());
    };
    if evidence.at > now {
        return Ok(());
    }
    let pm = pm_state::load(&ctx.store)?.context("bound PM disappeared during quota assessment")?;
    let Some(source) = pm
        .state
        .current_generation()
        .and_then(|g| g.connection.as_ref())
    else {
        return pause(ctx, n, PauseReason::LegacyBinding);
    };
    let profile = ctx.store.config_layers()?.config()?.pm.agent;
    if policy::approval(ctx, &profile, AgentRole::Pm).is_err() {
        return pause(ctx, n, PauseReason::PolicyNotApproved);
    }
    let target = match policy::target(ctx, &profile, AgentRole::Pm, source) {
        Ok(target) => target,
        Err(_) => return pause(ctx, n, PauseReason::NoApprovedCandidate),
    };
    // Pre-check before creating the financial root: a blocker leaves native state byte-equal.
    if !crate::pm_rotation::survey(ctx, n, false)?.0.is_empty() {
        return pause(ctx, n, PauseReason::EnteringEffect);
    }
    if let Err(reason) = checkpoint::quiescent(&pm.state, n) {
        return pause(ctx, n, reason);
    }
    if super::save_native_context(ctx)
        .and_then(|path| std::fs::read(path).context("reading native PM continuation context"))
        .is_err()
    {
        return pause(ctx, n, PauseReason::MissingCheckpoint);
    }
    if policy::ensure_root(ctx, &ContainerRef::Pm, &profile, &subject()).is_err() {
        return pause(ctx, n, PauseReason::PolicyNotApproved);
    }
    if crate::agent::hooks::pm::preflight(ctx, Some(&target.resolved)).is_err() {
        return pause(ctx, n, PauseReason::UnsupportedHost);
    }
    let decide = || frozen(ctx, &pm, &profile, &target, &evidence.summary);
    match super::successor(
        ctx,
        &pm,
        "subscription_quota",
        true,
        Some(&decide),
        Some(&target.resolved),
    ) {
        Ok(_) => Ok(()),
        Err(error) => {
            pause(ctx, n, PauseReason::SuccessorAmbiguous)?;
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
        pause(ctx, n, reason)?;
        Ok(false)
    };
    if !crate::pm_rotation::survey(ctx, n, false)?.0.is_empty() {
        return refuse(PauseReason::EnteringEffect);
    }
    let pm = pm_state::load(&ctx.store)?.context("PM disappeared during continuation")?;
    if pm.state.binding.generation != n {
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
    if continuation::prepare(
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
    )
    .is_err()
    {
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
pub(crate) fn regenerate(ctx: &Ctx) -> Result<()> {
    if let Err(error) = continuation::regenerate(ctx, &ContainerRef::Pm) {
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

pub(crate) fn require_recoverable(ctx: &Ctx, state: &AgentRunState) -> Result<()> {
    let n = state.binding.generation;
    for generation in [n, n.saturating_add(1)] {
        if let Some(intent) = continuation::intent_for(&ctx.store, &ContainerRef::Pm, generation)?
            && matches!(
                intent.phase,
                IntentPhase::Prepared | IntentPhase::Paused | IntentPhase::Closed
            )
        {
            pause(ctx, n, PauseReason::SuccessorAmbiguous)?;
            anyhow::bail!("PM continuation successor never bound; owner intervention required");
        }
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
#[path = "pm_continuation_tests.rs"]
mod tests;
