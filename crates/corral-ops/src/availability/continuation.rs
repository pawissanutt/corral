//! Native role continuation authority. An intent is published with the financial fence;
//! a bound host cannot spend until a separate atomic activation publishes Active.
use super::checkpoint;
use crate::Ctx;
use anyhow::{Context, Result, bail, ensure};
use corral_core::agent::{
    AgentRole, AgentRunState, ContainerRef, GenerationFence, HostHealth, pm_state,
};
use corral_core::connection::ConnectionBinding;
use corral_core::paid_episode::{PaidBinding, PaidEpisode, PaidLink, PaidPhase};
use corral_core::records::RecordArea;
use corral_core::{Store, agent};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[path = "continuation_close.rs"]
mod closure;
pub(crate) use closure::{close, close_in, close_task_in};
#[path = "continuation_index.rs"]
mod index;
pub(crate) use index::{from_source, latest_for, task_intent};

#[path = "continuation_start.rs"]
mod startup;
pub(crate) use startup::{
    activate_ready, automation_may_start, failed_start, require_automation_start,
};

#[path = "continuation_api_start.rs"]
mod api_start;
#[path = "continuation_context.rs"]
mod context_cache;
pub(crate) use api_start::prepare_launch;

pub(crate) const INTENTS: RecordArea<ContinuationIntent> = RecordArea::new("role-continuations");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum ContextSource {
    /// The checkpoint manifest digest; the hello notice instead exposes the artifact
    /// SHA-256 in context.digest so a host can verify the bytes at context.path.
    Checkpoint {
        digest: String,
    },
    FreshReadOnly {
        reason: String,
    },
    ResumedSession,
    ApiStart,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IntentPhase {
    Prepared,
    Active,
    Paused,
    Closed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PauseReason {
    PolicyNotApproved,
    NoApprovedCandidate,
    SwitchAlreadyUsed,
    LegacyBinding,
    MissingCheckpoint,
    CheckpointInvalid,
    SubjectChanged,
    UncertainEffect,
    EnteringEffect,
    TerminalResult,
    SuccessorAmbiguous,
    IndependenceViolated,
    AuthorityDenied,
    UnsupportedHost,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubjectRevision {
    pub task: Option<String>,
    pub spec_revision: Option<String>,
    pub target: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContinuationIntent {
    pub version: u32,
    pub role: AgentRole,
    pub source: PaidBinding,
    pub source_link: PaidLink,
    pub successor: PaidBinding,
    pub link: PaidLink,
    pub context: ContextSource,
    pub subject: SubjectRevision,
    pub reason: String,
    pub phase: IntentPhase,
    pub pause: Option<PauseReason>,
    pub created: Timestamp,
    pub updated: Timestamp,
}
pub(crate) struct Transfer {
    pub source: ContainerRef,
    pub successor: ContainerRef,
    pub target: ConnectionBinding,
    pub context: ContextSource,
    pub subject: SubjectRevision,
    pub reason: String,
}

/// Full materialization preserves native budget projections written by head-keyed paths.
/// Every writer opens a record unit; save_paid_episode selects CommitPaidEpisode for it.
pub(crate) fn with_state<R>(
    ctx: &Ctx,
    container: &ContainerRef,
    f: impl FnOnce(&mut AgentRunState) -> Result<R>,
) -> Result<R> {
    if let Some(task) = container.task() {
        let mut result = None;
        ctx.store.update(task, |task| {
            result = Some(f(agent::container_state_mut(task, container)
                .context("continuation container has no native state")?)?);
            Ok(())
        })?;
        return result.context("continuation state update did not run");
    }
    if let Some(session) = container.assistant() {
        return ctx
            .store
            .update_assistant_container(session, |a| f(&mut a.state));
    }
    // Include all families, notably turns, receipts, budget_requests and generations.
    // paid_episode is a scalar on the PM head and is always materialized.
    pm_state::update(&ctx.store, |pm| {
        let pm = pm.as_mut().context("continuation PM state missing")?;
        Ok((f(&mut pm.state)?, true))
    })
}

fn state_for(store: &Store, container: &ContainerRef) -> Result<Option<AgentRunState>> {
    if let Some(task) = container.task() {
        return Ok(agent::container_state(&store.get(task)?, container).cloned());
    }
    if let Some(session) = container.assistant() {
        return Ok(Some(store.get_assistant_container(session)?.state));
    }
    Ok(pm_state::load(store)?.map(|pm| pm.state))
}
fn key(container: &ContainerRef, generation: u32) -> Result<String> {
    Ok(agent::paths::sha256_hex(&serde_json::to_vec(&(
        container, generation,
    ))?))
}
pub(crate) fn intent_for(
    store: &Store,
    successor: &ContainerRef,
    generation: u32,
) -> Result<Option<ContinuationIntent>> {
    let intent = INTENTS.load(store, &key(successor, generation)?)?;
    if let Some(intent) = &intent {
        ensure!(
            intent.version == 1
                && intent.successor.container == *successor
                && intent.successor.generation == generation,
            "SuccessorAmbiguous"
        );
        index::validate(store, intent)?;
    }
    Ok(intent)
}
fn save(store: &Store, intent: &ContinuationIntent) -> Result<()> {
    INTENTS.save(
        store,
        &key(&intent.successor.container, intent.successor.generation)?,
        intent,
    )?;
    index::publish(store, intent)
}
fn approved(store: &Store, episode: &PaidEpisode, source: &ConnectionBinding) -> Result<()> {
    let config = store.config_layers()?.config()?;
    ensure!(
        episode.approval.policy_digest == config.profile_setup_revision()?,
        "PolicyNotApproved"
    );
    if source.connection.api_key() && api_start::approved(store, episode, source).is_ok() {
        return Ok(());
    }
    let mut refusal = None;
    for (profile, configured) in &config.profiles {
        let Some(policy) = configured
            .metadata
            .as_ref()
            .and_then(|m| m.fallback.as_ref())
        else {
            continue;
        };
        if !policy.automatic
            || !policy.paid_roles.contains(&episode.root.role)
            || policy.approval != episode.approval.approval
            || !policy.spending.as_ref().is_some_and(|s| {
                s.currency == "USD"
                    && s.maximum_micros.checked_mul(1000) == Some(episode.approval.allowance_nanos)
            })
        {
            continue;
        }
        match super::policy::source_position(&config, profile, source) {
            Ok(_) => return Ok(()),
            Err(error) => refusal = Some(error),
        }
    }
    Err(refusal.unwrap_or_else(|| anyhow::anyhow!("PolicyNotApproved")))
}
fn subject_current(
    store: &Store,
    container: &ContainerRef,
    subject: &SubjectRevision,
) -> Result<()> {
    ensure!(
        subject.task.as_deref() == container.task(),
        "SubjectChanged"
    );
    if let Some(task) = container.task() {
        let task = store.get(task)?;
        if let Some(revision) = &subject.spec_revision {
            let current = match container {
                ContainerRef::Planner { .. } | ContainerRef::PlanReviewer { .. } => {
                    crate::plan_spec_revision(&task)?
                }
                _ => corral_core::store::spec_revision(&task)?,
            };
            ensure!(&current == revision, "SubjectChanged");
        }
        match container {
            ContainerRef::Reviewer { run, .. } => {
                let review = task
                    .runs
                    .iter()
                    .find(|r| &r.id == run)
                    .and_then(|r| r.review.as_ref())
                    .context("SubjectChanged")?;
                ensure!(
                    subject.target.as_deref() == Some(review.sha.as_str()),
                    "SubjectChanged"
                );
            }
            ContainerRef::PlanReviewer { plan, .. } => {
                ensure!(
                    subject.target.as_deref() == Some(plan.as_str())
                        && task.plans.iter().any(|p| &p.id == plan),
                    "SubjectChanged"
                );
            }
            _ => {}
        }
    }
    Ok(())
}
fn same_lineage(source: &ContainerRef, successor: &ContainerRef) -> bool {
    match (source, successor) {
        (ContainerRef::Worker { task: a, .. }, ContainerRef::Worker { task: b, .. })
        | (ContainerRef::Planner { task: a, .. }, ContainerRef::Planner { task: b, .. }) => {
            a == b && source != successor
        }
        (
            ContainerRef::Reviewer {
                task: a, run: ar, ..
            },
            ContainerRef::Reviewer {
                task: b, run: br, ..
            },
        ) => a == b && ar == br && source != successor,
        (
            ContainerRef::PlanReviewer {
                task: a, plan: ap, ..
            },
            ContainerRef::PlanReviewer {
                task: b, plan: bp, ..
            },
        ) => a == b && ap == bp && source != successor,
        (ContainerRef::Pm, ContainerRef::Pm) => true,
        (ContainerRef::Assistant { session: a }, ContainerRef::Assistant { session: b }) => a == b,
        _ => false,
    }
}
fn context_valid(store: &Store, intent: &ContinuationIntent) -> Result<()> {
    match &intent.context {
        ContextSource::ApiStart => ensure!(
            intent.source == intent.successor && intent.source.binding.connection.api_key(),
            "invalid API start context"
        ),
        ContextSource::Checkpoint { digest } => {
            checkpoint::load(store, digest, &intent.source, &intent.subject)?;
        }
        ContextSource::FreshReadOnly { .. } => ensure!(
            matches!(
                intent.role,
                AgentRole::Reviewer | AgentRole::Planner | AgentRole::PlanReviewer
            ),
            "MissingCheckpoint"
        ),
        ContextSource::ResumedSession => ensure!(
            intent.source.container == intent.successor.container
                && intent.source.binding == intent.successor.binding
                && intent.source.binding.connection.api_key(),
            "MissingCheckpoint"
        ),
    }
    Ok(())
}
fn event(
    store: &Store,
    kind: &str,
    intent: &ContinuationIntent,
    episode: &PaidEpisode,
) -> Result<()> {
    store.log_event(intent.successor.container.task(), kind, json!({
        "role": intent.role, "source": intent.source.container, "successor": intent.successor.container,
        "source_connection": intent.source.binding.connection, "target_connection": intent.successor.binding.connection,
        "reason": intent.reason, "candidate": intent.successor.binding.candidate,
        "episode": episode.id, "allowance_nanos": episode.approval.allowance_nanos,
        "consumed_or_reserved_nanos": episode.ledger.totals().1, "pause": intent.pause,
    }))
}

/// Fence source, advance the financial epoch and insert the sole successor in ONE commit.
pub(crate) fn prepare(ctx: &Ctx, mut transfer: Transfer) -> Result<ContinuationIntent> {
    if !startup::session_may_start(ctx, &transfer.source)?
        || !startup::session_may_start(ctx, &transfer.successor)?
    {
        return Err(crate::operating::AuthorizationRefused(
            "Held: continuation session paused or stopped".into(),
        )
        .into());
    }
    ensure!(
        same_lineage(&transfer.source, &transfer.successor),
        "SuccessorAmbiguous"
    );
    transfer.target.validate()?;
    // These are trusted descriptive inputs, not durable error/log channels.
    let _ = transfer.reason;
    if let ContextSource::FreshReadOnly { reason } = &mut transfer.context {
        *reason = "fresh read-only assessment".into();
    }
    with_state(ctx, &transfer.source, |state| {
        // Search by predecessor before any mutation, including after a retained generation
        // advanced. A different successor or checkpoint must not rewrite any durable bytes.
        let previous = from_source(&ctx.store, &transfer.source)?.filter(|i| {
            // An API start has no predecessor: its self-source is the incoming
            // admission, not an already-selected outgoing recovery generation.
            i.context != ContextSource::ApiStart
                && (i.source.generation == state.binding.generation
                    || i.phase == IntentPhase::Prepared
                        && state.paid_episode.as_ref() == Some(&i.link))
        });
        if let Some(intent) = previous {
            return index::matching(
                intent,
                &transfer.source,
                &transfer.successor,
                &transfer.target,
                &transfer.subject,
                &transfer.context,
            );
        }
        let source = PaidBinding {
            container: transfer.source.clone(),
            generation: state.binding.generation,
            binding: state
                .current_generation()
                .and_then(|g| g.connection.clone())
                .context("LegacyBinding")?,
        };
        let source_link = state.paid_episode.clone().context("PolicyNotApproved")?;
        let mut episode = ctx.store.paid_episode(&source_link.episode)?;
        // The financial row is canonical if an index is absent. Never interpret a lost
        // pointer as permission to prepare a second successor.
        if let PaidPhase::Prepared { successor, .. } = &episode.phase {
            let intent = intent_for(&ctx.store, &successor.container, successor.generation)?
                .context("SuccessorAmbiguous: prepared episode has no indexed intent")?;
            return index::matching(
                intent,
                &transfer.source,
                &transfer.successor,
                &transfer.target,
                &transfer.subject,
                &transfer.context,
            );
        }
        approved(&ctx.store, &episode, &source.binding)?;
        ensure!(
            episode.ledger.totals().1 < episode.approval.allowance_nanos,
            "paid allowance exhausted; restart cannot refresh allowance"
        );
        ensure!(!state.binding.closing, "TerminalResult");
        subject_current(&ctx.store, &source.container, &transfer.subject)?;
        if matches!(transfer.context, ContextSource::ResumedSession) {
            checkpoint::resumed_quiescent(state, source.generation)
                .map_err(|r| anyhow::anyhow!("{r:?}"))?;
        } else {
            checkpoint::quiescent(state, source.generation)
                .map_err(|r| anyhow::anyhow!("{r:?}"))?;
        }
        let generation = if transfer.source == transfer.successor {
            source
                .generation
                .checked_add(1)
                .context("continuation generation overflow")?
        } else {
            1
        };
        let successor = PaidBinding {
            container: transfer.successor.clone(),
            generation,
            binding: transfer.target,
        };
        ensure!(
            intent_for(&ctx.store, &successor.container, generation)?.is_none(),
            "SuccessorAmbiguous"
        );
        // Only transport-classified summaries enter durable records. Caller error strings
        // are intentionally not retained: they may contain SDK dumps or credentials.
        let reason = state
            .turns
            .iter()
            .filter(|t| t.generation == source.generation)
            .max_by_key(|t| t.seq)
            .and_then(|t| t.provider_failure.as_ref())
            .map(|f| f.summary())
            .unwrap_or_else(|| "paid session recovery".into());
        let now = corral_core::clock::now();
        let mut intent = ContinuationIntent {
            version: 1,
            role: source.role(),
            source,
            source_link: source_link.clone(),
            successor,
            link: source_link.clone(),
            context: transfer.context,
            subject: transfer.subject,
            reason,
            phase: IntentPhase::Prepared,
            pause: None,
            created: now,
            updated: now,
        };
        context_valid(&ctx.store, &intent)?;
        if matches!(intent.context, ContextSource::ResumedSession) {
            ensure!(
                matches!(intent.role, AgentRole::Pm | AgentRole::Assistant),
                "AuthorityDenied"
            );
            ensure!(
                matches!(state.binding.health, HostHealth::Lost | HostHealth::Fenced),
                "AuthorityDenied"
            );
        } else {
            ensure!(
                super::policy::quota_evidence(
                    state,
                    state
                        .turns
                        .iter()
                        .filter(|t| t.generation == state.binding.generation)
                        .max_by_key(|t| t.seq)
                )
                .is_some(),
                "NoApprovedCandidate"
            );
        }
        intent.link = episode.prepare_transfer(
            &source_link,
            &intent.source,
            intent.successor.clone(),
            state,
        )?;
        ctx.store.save_paid_episode(&episode)?;
        save(&ctx.store, &intent)?;
        event(&ctx.store, "continuation_prepared", &intent, &episode)?;
        Ok(intent)
    })
}

/// Only retained paid containers may recover their host inside the sticky API episode.
pub(crate) fn regenerate(
    ctx: &Ctx,
    container: &ContainerRef,
) -> Result<Option<ContinuationIntent>> {
    ensure!(
        matches!(container, ContainerRef::Pm | ContainerRef::Assistant { .. }),
        "AuthorityDenied"
    );
    let Some(state) = state_for(&ctx.store, container)? else {
        return Ok(None);
    };
    let Some(binding) = state
        .current_generation()
        .and_then(|g| g.connection.clone())
    else {
        return Ok(None);
    };
    if !binding.connection.api_key() || state.paid_episode.is_none() {
        return Ok(None);
    }
    let current = intent_for(&ctx.store, container, state.binding.generation)?
        .context("SuccessorAmbiguous")?;
    ensure!(current.phase == IntentPhase::Active, "SuccessorAmbiguous");
    prepare(
        ctx,
        Transfer {
            source: container.clone(),
            successor: container.clone(),
            target: binding,
            context: ContextSource::ResumedSession,
            subject: current.subject,
            reason: "paid session recovery".into(),
        },
    )
    .map(Some)
}

pub(crate) fn require_launch(
    store: &Store,
    container: &ContainerRef,
    binding: &ConnectionBinding,
) -> Result<()> {
    binding.validate()?;
    if !binding.connection.api_key() {
        if let Some(state) = state_for(store, container)?
            && let Some(link) = &state.paid_episode
        {
            // A prepared policy root on a subscription source is not a paid switch.
            // Refusals before transfer must still be able to resume that subscription.
            ensure!(
                state
                    .current_generation()
                    .and_then(|g| g.connection.as_ref())
                    .is_none_or(|b| !b.connection.api_key())
                    && store.paid_episode(&link.episode)?.switches == 0,
                "paid route is sticky; start a new session to change billing connection"
            );
        }
        return binding.connection.require_admission();
    }
    let generation = next_generation(store, container)?;
    let intent = intent_for(store, container, generation)?.context("paid_admission_required")?;
    ensure!(
        intent.phase == IntentPhase::Prepared && intent.successor.binding == *binding,
        "paid_admission_required"
    );
    let episode = store.paid_episode(&intent.link.episode)?;
    approved(store, &episode, &intent.source.binding)?;
    ensure!(
        episode.ledger.totals().1 < episode.approval.allowance_nanos,
        "paid allowance exhausted; cannot refresh allowance by restarting"
    );
    ensure!(
        episode.epoch == intent.link.epoch
            && matches!(&episode.phase,
        PaidPhase::Prepared { successor, .. } if successor == &intent.successor),
        "paid_admission_required"
    );
    subject_current(store, container, &intent.subject)?;
    context_valid(store, &intent)
}
fn next_generation(store: &Store, container: &ContainerRef) -> Result<u32> {
    state_for(store, container)?.map_or(Ok(1), |s| {
        s.generations
            .iter()
            .map(|g| g.n)
            .max()
            .unwrap_or(0)
            .max(s.retired.as_ref().map_or(0, |r| r.floor.saturating_sub(1)))
            .checked_add(1)
            .context("continuation generation overflow")
    })
}
pub(crate) fn require_preflight(
    store: &Store,
    role: AgentRole,
    binding: &ConnectionBinding,
    profile: &str,
) -> Result<()> {
    binding.validate()?;
    if !binding.connection.api_key() {
        return binding.connection.require_admission();
    }
    if store
        .config_layers()?
        .config()?
        .profiles
        .get(profile)
        .is_some_and(|p| p.connection.is_some_and(|c| c.api_key()))
    {
        api_start::for_profile(store, profile, role, binding)?;
        return Ok(());
    }
    for row in store.record_rows(corral_core::paid_episode::AREA)? {
        let episode = store.paid_episode(&row.key)?;
        let route = match &episode.phase {
            PaidPhase::Active { owner } => &owner.binding,
            PaidPhase::Prepared { successor, .. } => &successor.binding,
            PaidPhase::Closed => continue,
        };
        if episode.root.role != role
            || approved(store, &episode, route).is_err()
            || !episode.approval.candidates.contains(binding)
        {
            continue;
        }
        if route == binding || !route.connection.api_key() && episode.switches == 0 {
            return Ok(());
        }
    }
    bail!(
        "paid_admission_required: configure metadata.api_start roles, finite USD spending and explicit approval for this exact preferred API route"
    )
}

/// Intent ContextSource::Checkpoint.digest identifies the immutable manifest; the returned
/// context.digest is the artifact SHA-256, checked by the host against context.path bytes.
pub(crate) fn notice(
    store: &Store,
    container: &ContainerRef,
    generation: u32,
) -> Result<Option<Value>> {
    let Some(intent) = intent_for(store, container, generation)? else {
        return Ok(None);
    };
    if intent.phase != IntentPhase::Prepared {
        return Ok(None);
    }
    let episode = store.paid_episode(&intent.link.episode)?;
    approved(store, &episode, &intent.source.binding)?;
    ensure!(
        episode.epoch == intent.link.epoch
            && matches!(&episode.phase,
        PaidPhase::Prepared { successor, .. } if successor == &intent.successor),
        "SuccessorAmbiguous"
    );
    let context = match &intent.context {
        ContextSource::Checkpoint { digest } => {
            let (checkpoint, bytes) =
                checkpoint::load(store, digest, &intent.source, &intent.subject)?;
            let path = context_cache::materialize(store, &intent, &checkpoint, &bytes)?;
            json!({"kind":"checkpoint", "digest":checkpoint.context.blob.hash().as_str(),
                "path":path, "bytes":checkpoint.context.bytes})
        }
        other => {
            context_valid(store, &intent)?;
            serde_json::to_value(other)?
        }
    };
    Ok(Some(json!({"version":1, "generation":generation,
        "binding_digest":intent.successor.binding.digest,
        "source_connection":intent.source.binding.connection, "target_connection":intent.successor.binding.connection,
        "candidate":intent.successor.binding.candidate, "reason":intent.reason, "episode":episode.id,
        "allowance_nanos":episode.approval.allowance_nanos,
        "consumed_or_reserved_nanos":episode.ledger.totals().1, "context":context})))
}
pub(crate) fn activate(ctx: &Ctx, container: &ContainerRef, generation: u32) -> Result<bool> {
    // Ordinary hellos have no financial intent. Keep their keyed admission path
    // independent of settled task history; this function grants nothing without an intent.
    let Some(intent) = intent_for(&ctx.store, container, generation)? else {
        return Ok(false);
    };
    if intent.phase == IntentPhase::Prepared && !automation_may_start(ctx, container)? {
        return Ok(false);
    }
    match intent.phase {
        IntentPhase::Prepared => {}
        IntentPhase::Active => {
            let episode = ctx.store.paid_episode(&intent.link.episode)?;
            approved(&ctx.store, &episode, &intent.source.binding)?;
            episode.owner(&intent.link, &intent.successor)?;
            return Ok(false);
        }
        IntentPhase::Paused | IntentPhase::Closed => bail!("SuccessorAmbiguous"),
    }
    with_state(ctx, container, |state| {
        let mut intent =
            intent_for(&ctx.store, container, generation)?.context("SuccessorAmbiguous")?;
        let mut episode = ctx.store.paid_episode(&intent.link.episode)?;
        approved(&ctx.store, &episode, &intent.source.binding)?;
        if intent.phase == IntentPhase::Active {
            episode.owner(&intent.link, &intent.successor)?;
            return Ok(false);
        }
        ensure!(intent.phase == IntentPhase::Prepared, "SuccessorAmbiguous");
        if !startup::session_may_start(ctx, container)?
            || !startup::session_may_start(ctx, &intent.source.container)?
        {
            return Err(crate::operating::AuthorizationRefused(
                "Held: continuation session paused or stopped".into(),
            )
            .into());
        }
        ensure!(!state.binding.closing, "TerminalResult");
        subject_current(&ctx.store, container, &intent.subject)?;
        context_valid(&ctx.store, &intent)?;
        if matches!(intent.context, ContextSource::ResumedSession) {
            let previous = state
                .generation(intent.source.generation)
                .and_then(|g| g.bound.as_ref())
                .context("MissingCheckpoint")?;
            let next = state
                .generation(generation)
                .and_then(|g| g.bound.as_ref())
                .context("MissingCheckpoint")?;
            let empty_start = state.turns.is_empty()
                && state.retired.is_none()
                && episode.ledger.requests.is_empty();
            ensure!(
                empty_start
                    || previous.session_id == next.session_id
                        && previous.session_file == next.session_file,
                "MissingCheckpoint"
            );
        }
        episode.activate(&intent.link, &intent.successor, state)?;
        intent.phase = IntentPhase::Active;
        intent.updated = corral_core::clock::now();
        ctx.store.save_paid_episode(&episode)?;
        save(&ctx.store, &intent)?;
        event(&ctx.store, "continuation_switched", &intent, &episode)?;
        Ok(true)
    })
}

pub(crate) fn pause(
    ctx: &Ctx,
    container: &ContainerRef,
    reason: PauseReason,
    _detail: &str,
) -> Result<()> {
    let intent = latest_for(&ctx.store, container)?.filter(|i| i.phase != IntentPhase::Closed);
    let pending_generation = intent
        .as_ref()
        .filter(|i| i.successor.container == *container)
        .map(|i| i.successor.generation);
    let needs_fence = |state: &AgentRunState| {
        state
            .current_generation()
            .and_then(|g| g.connection.as_ref())
            .is_some_and(|b| b.connection.api_key())
            || pending_generation == Some(state.binding.generation)
    };
    let write = |state: Option<&mut AgentRunState>, already_closed: bool| -> Result<()> {
        if let Some(state) = state
            && needs_fence(state)
        {
            if !already_closed {
                close_in(&ctx.store, container, state, "continuation paused")?;
            }
            state.binding.health = HostHealth::Fenced;
            if let Some(g) = state.generation_mut(state.binding.generation) {
                g.fenced = Some(GenerationFence {
                    reason: "continuation paused".into(),
                    at: corral_core::clock::now(),
                });
            }
        }
        if let Some(mut intent) = intent {
            intent.phase = IntentPhase::Paused;
            intent.pause = Some(reason);
            intent.updated = corral_core::clock::now();
            let episode = ctx.store.paid_episode(&intent.link.episode)?;
            save(&ctx.store, &intent)?;
            context_cache::remove(&ctx.store, &intent)?;
            event(&ctx.store, "continuation_paused", &intent, &episode)
        } else {
            ctx.store.log_event(
                container.task(),
                "continuation_paused",
                json!({"container":container, "pause":reason}),
            )
        }
    };
    if let Some(task) = container.task() {
        ctx.store.update(task, |task| {
            if agent::container_state(task, container).is_some_and(&needs_fence) {
                close_task_in(&ctx.store, container, task, "continuation paused")?;
            }
            write(agent::container_state_mut(task, container), true)
        })?;
        return Ok(());
    }
    if state_for(&ctx.store, container)?.is_some() {
        with_state(ctx, container, |s| write(Some(s), false))
    } else {
        ctx.store.records_transaction(|| write(None, false))
    }
}
pub(crate) fn prepared_candidate(
    store: &Store,
    task: &str,
    role: AgentRole,
) -> Result<Option<String>> {
    let Some(intent) = task_intent(store, task, role)?.filter(|i| i.phase == IntentPhase::Prepared)
    else {
        return Ok(None);
    };
    let episode = store.paid_episode(&intent.link.episode)?;
    approved(store, &episode, &intent.source.binding)?;
    ensure!(
        episode.epoch == intent.link.epoch
            && matches!(&episode.phase, PaidPhase::Prepared { successor, .. } if successor == &intent.successor),
        "SuccessorAmbiguous"
    );
    Ok(Some(
        intent
            .successor
            .binding
            .candidate
            .context("NoApprovedCandidate")?,
    ))
}
