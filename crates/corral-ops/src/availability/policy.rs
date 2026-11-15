//! Explicit, frozen spending policy for native role continuations. Credentials are neither
//! approval nor quota evidence. A root belongs to the first attempt, never its successor;
//! retained PM/assistant sessions keep one allowance even after closure or exhaustion.
use super::continuation::{self, SubjectRevision};
use crate::Ctx;
use anyhow::{Context, Result, ensure};
use corral_core::{
    Config, ExecutionCandidate, ResolvedProfile, Store,
    agent::{AgentRole, AgentRunState, AgentTurn, ContainerRef, HostHealth, TurnClass, paths},
    availability::AvailabilityClass,
    connection::{BillingMode, ConnectionBinding},
    paid_episode::{PaidApproval, PaidBinding, PaidLink, PaidPhase, PaidRoot},
};
use jiff::Timestamp;

pub(crate) struct Target {
    pub candidate: ExecutionCandidate,
    pub resolved: ResolvedProfile,
    pub binding: ConnectionBinding,
}

pub(crate) struct QuotaEvidence {
    pub generation: u32,
    pub turn: u64,
    pub at: Timestamp,
    pub summary: String,
}

/// In-memory eligibility signal only; spending approval is revalidated at the boundary.
pub(crate) fn profile_configured(config: &Config, profile: &str, role: AgentRole) -> bool {
    config
        .profiles
        .get(profile)
        .and_then(|p| p.metadata.as_ref())
        .and_then(|m| m.fallback.as_ref())
        .is_some_and(|f| f.paid_roles.contains(&role))
}

pub(crate) fn configured(config: &Config, role: AgentRole) -> bool {
    config
        .profiles
        .keys()
        .any(|profile| profile_configured(config, profile, role))
}

/// Frozen approval from the role profile's fallback; refusal never chooses another policy.
pub(crate) fn approval(ctx: &Ctx, profile: &str, role: AgentRole) -> Result<PaidApproval> {
    approved(&ctx.store.config_layers()?.config()?, profile, role)
}

fn approved(config: &Config, profile: &str, role: AgentRole) -> Result<PaidApproval> {
    let metadata = config
        .profiles
        .get(profile)
        .and_then(|p| p.metadata.as_ref())
        .context("PolicyNotApproved: paid fallback needs profile metadata")?;
    let fallback = metadata
        .fallback
        .as_ref()
        .context("PolicyNotApproved: no paid fallback approval")?;
    ensure!(
        fallback.automatic && fallback.max_switches > 0 && fallback.paid_roles.contains(&role),
        "PolicyNotApproved: automatic paid fallback is not approved for {}",
        role.as_str()
    );
    let spending = fallback
        .spending
        .as_ref()
        .context("PolicyNotApproved: paid fallback needs a finite USD allowance")?;
    ensure!(
        spending.currency == "USD",
        "PolicyNotApproved: paid allowance must be in USD"
    );
    let allowance_nanos = spending
        .maximum_micros
        .checked_mul(1000)
        .context("PolicyNotApproved: paid allowance overflows nanos")?;
    let mut candidates = Vec::new();
    for candidate in &metadata.candidates {
        if !corral_core::paid_fallback::is_api(candidate) || fallback.admits(candidate).is_err() {
            continue;
        }
        let binding = config
            .resolve_candidate(profile, candidate)?
            .connection_binding()?
            .context("NoApprovedCandidate: API candidate needs a native binding")?;
        candidates.push(binding);
    }
    let approval = PaidApproval {
        policy_digest: config.profile_setup_revision()?,
        approval: fallback.approval.clone(),
        allowance_nanos,
        candidates,
    };
    approval.validate()?;
    Ok(approval)
}

/// Locate the source without rewriting its recorded binding. Profile launches omit the
/// candidate id; only subscription sources may then match one unique runtime identity.
/// Both bindings validate first. Version stays exact, and origin stays exact even though
/// it is derived from identity; only candidate and its derived digest may differ.
pub(crate) fn source_position(
    config: &Config,
    profile: &str,
    source: &ConnectionBinding,
) -> Result<usize> {
    source.validate()?;
    let metadata = config
        .profiles
        .get(profile)
        .and_then(|p| p.metadata.as_ref())
        .context("LegacyBinding: source profile has no candidates")?;
    let mut position = None;
    for (index, candidate) in metadata.candidates.iter().enumerate() {
        let Some(binding) = config
            .resolve_candidate(profile, candidate)?
            .connection_binding()?
        else {
            continue;
        };
        let matches = if source.candidate.is_some() {
            &binding == source
        } else {
            source.connection.billing() == BillingMode::Subscription
                && binding.connection.billing() == BillingMode::Subscription
                && binding.version == source.version
                && binding.origin == source.origin
                && binding.connection == source.connection
                && binding.identity == source.identity
                && binding.model == source.model
                && binding.effort == source.effort
        };
        if matches {
            ensure!(
                position.is_none(),
                "LegacyBinding: source matches several metadata candidates"
            );
            position = Some(index);
        }
    }
    position.context("LegacyBinding: source candidate is missing")
}

/// Exactly one approved same-vendor API candidate after the uniquely identified source.
pub(crate) fn target(
    ctx: &Ctx,
    profile: &str,
    role: AgentRole,
    source: &ConnectionBinding,
) -> Result<Target> {
    ensure!(
        source.connection.billing() == BillingMode::Subscription,
        "NoApprovedCandidate: paid fallback needs a subscription source"
    );
    let config = ctx.store.config_layers()?.config()?;
    let approval = approved(&config, profile, role)?;
    let metadata = config
        .profiles
        .get(profile)
        .and_then(|p| p.metadata.as_ref())
        .context("PolicyNotApproved: paid profile disappeared")?;
    let position = source_position(&config, profile, source)?;
    let mut selected = None;
    for candidate in metadata.candidates.iter().skip(position + 1) {
        let resolved = config.resolve_candidate(profile, candidate)?;
        let Some(binding) = resolved.connection_binding()? else {
            continue;
        };
        if !binding.connection.api_key()
            || binding.connection.vendor() != source.connection.vendor()
            || !approval.candidates.contains(&binding)
        {
            continue;
        }
        ensure!(
            selected.is_none(),
            "NoApprovedCandidate: several same-vendor API candidates follow the source"
        );
        selected = Some(Target {
            candidate: candidate.clone(),
            resolved,
            binding,
        });
    }
    selected.context("NoApprovedCandidate: no approved same-vendor API target after source")
}

fn role(container: &ContainerRef) -> AgentRole {
    match container {
        ContainerRef::Worker { .. } => AgentRole::Worker,
        ContainerRef::Reviewer { .. } => AgentRole::Reviewer,
        ContainerRef::Planner { .. } => AgentRole::Planner,
        ContainerRef::PlanReviewer { .. } => AgentRole::PlanReviewer,
        ContainerRef::Pm => AgentRole::Pm,
        ContainerRef::Assistant { .. } => AgentRole::Assistant,
    }
}

pub(crate) fn root_for(
    store: &Store,
    container: &ContainerRef,
    subject: &SubjectRevision,
) -> Result<PaidRoot> {
    ensure!(
        subject
            .task
            .as_deref()
            .is_none_or(|task| Some(task) == container.task()),
        "SubjectChanged: paid root names a different task"
    );
    let (subject, attempt) = match container {
        ContainerRef::Worker { task, run } => (task.clone(), run.clone()),
        ContainerRef::Reviewer { task, run, review } => {
            let sha = subject
                .target
                .as_deref()
                .filter(|sha| !sha.is_empty())
                .context("SubjectChanged: paid review root needs the exact review sha")?;
            (format!("{task}/{run}@{sha}"), review.clone())
        }
        ContainerRef::Planner { task, run } => {
            let revision = subject
                .spec_revision
                .as_deref()
                .filter(|revision| !revision.is_empty())
                .context("SubjectChanged: paid planning root needs the spec revision")?;
            (format!("{task}@{revision}"), run.clone())
        }
        ContainerRef::PlanReviewer { task, plan, review } => {
            (format!("{task}/{plan}"), review.clone())
        }
        ContainerRef::Pm => ("pm".into(), "1".into()),
        ContainerRef::Assistant { session } => (session.clone(), "1".into()),
    };
    Ok(PaidRoot {
        store: paths::store_id(store),
        role: role(container),
        subject,
        attempt,
    })
}

/// Recover or create once, then activate only on the exact healthy bound source, atomically
/// with its native link. Existing episodes keep their approval, reservations and epoch.
pub(crate) fn ensure_root(
    ctx: &Ctx,
    container: &ContainerRef,
    profile: &str,
    subject: &SubjectRevision,
) -> Result<PaidLink> {
    let config = ctx.store.config_layers()?.config()?;
    let approval = approved(&config, profile, role(container))?;
    continuation::with_state(ctx, container, |state| {
        let generation = state
            .current_generation()
            .context("LegacyBinding: paid source generation is missing")?;
        let binding = generation
            .connection
            .as_ref()
            .context("LegacyBinding: paid source needs an explicit connection binding")?;
        binding.validate()?;
        source_position(&config, profile, binding)?;
        ensure!(
            state.binding.health == HostHealth::Healthy
                && !state.binding.closing
                && state.binding.role == role(container)
                && state.binding.store == paths::store_id(&ctx.store)
                && generation.bound.is_some()
                && generation.fenced.is_none(),
            "paid root activation needs the exact healthy bound source"
        );
        let source = PaidBinding {
            container: container.clone(),
            generation: generation.n,
            binding: binding.clone(),
        };
        let mut episode = if let Some(link) = &state.paid_episode {
            let episode = ctx.store.paid_episode(&link.episode)?;
            episode.owner(link, &source)?;
            episode
        } else {
            ensure!(
                !source.binding.connection.api_key()
                    && continuation::intent_for(&ctx.store, container, source.generation)?
                        .is_none(),
                "paid successor must inherit its financial root; cannot initialize a new allowance"
            );
            let root = root_for(&ctx.store, container, subject)?;
            match ctx.store.paid_episode_for_root(&root)? {
                Some(episode) => episode,
                None => {
                    let link =
                        ctx.store
                            .create_paid_episode(root, approval.clone(), source.clone())?;
                    ctx.store.paid_episode(&link.episode)?
                }
            }
        };
        ensure!(
            episode.approval == approval,
            "PolicyNotApproved: paid root approval is frozen; policy changed"
        );
        ensure!(
            !matches!(episode.phase, PaidPhase::Closed)
                && episode.ledger.totals().1 < episode.approval.allowance_nanos,
            "paid root is closed or exhausted; cannot initialize a new allowance"
        );
        let link = PaidLink {
            episode: episode.id.clone(),
            epoch: episode.epoch,
        };
        match &episode.phase {
            PaidPhase::Prepared { successor, carry } => {
                ensure!(
                    successor == &source && carry.is_none() && episode.switches == 0,
                    "paid root recovery cannot activate a transferred successor"
                );
                episode.activate(&link, &source, state)?;
            }
            PaidPhase::Active { .. } => {
                episode.owner(&link, &source)?;
                state.paid_episode = Some(link.clone());
            }
            PaidPhase::Closed => anyhow::bail!("paid root is closed"),
        }
        ctx.store.save_paid_episode(&episode)?;
        Ok(link)
    })
}

/// Only sanitized native quota evidence from the latest ended current-generation turn on
/// that generation's subscription binding is eligible; legacy prose is never evidence.
pub(crate) fn quota_evidence(
    state: &AgentRunState,
    latest: Option<&AgentTurn>,
) -> Option<QuotaEvidence> {
    let generation = state.current_generation()?;
    let binding = generation.connection.as_ref()?;
    if binding.validate().is_err() || binding.connection.billing() != BillingMode::Subscription {
        return None;
    }
    let turn = state
        .turns
        .iter()
        .chain(latest)
        .filter(|turn| turn.generation == generation.n && turn.ended.is_some())
        .max_by_key(|turn| turn.seq)?;
    if matches!(state.binding.role, AgentRole::Worker | AgentRole::Reviewer)
        && turn.class != Some(TurnClass::ProviderFailed)
    {
        return None;
    }
    let failure = turn.provider_failure.as_ref()?;
    if failure.connection != binding.connection
        || !matches!(
            failure.availability(),
            Some(AvailabilityClass::QuotaExhausted { .. })
        )
    {
        return None;
    }
    Some(QuotaEvidence {
        generation: generation.n,
        turn: turn.seq,
        at: turn.ended?,
        summary: failure.summary(),
    })
}
