//! Admission in contract order: bounds (envelope.rs), resolution of the NAMED container,
//! authentication against the named generation, receipt lookup, then role liveness, current
//! generation and saved pane for new mutations. Also the generation binding primitives.
use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use corral_core::agent::{
    self, AgentBinding, AgentGeneration, AgentPm, AgentRole, AgentRunState, CapabilitySnapshot,
    ContainerRef, GENERATIONS_MAX, GenerationFence, HostHealth, RequestBinding, RequestIdentity,
    RequestOrigin, RequestReceipt, paths, pm_state,
};
use corral_core::assistant::AssistantContainer;
use corral_core::{Store, Task, TaskHead, TaskRef};
use serde_json::json;

use super::call::{RejectCode, Rejection};
use super::envelope::Envelope;
use crate::Ctx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Mutation {
        settles_turn: Option<u64>,
    },
    Hello,
    /// receipt_lookup: authentication only; a fenced generation may still read its container.
    Lookup,
}

/// The snapshot admission resolved; the final transaction rereads it under its locks.
#[derive(Debug)]
pub(crate) enum Resolved {
    Task(Box<TaskHead>),
    Pm(Box<AgentPm>),
    /// An assistant session's container and whether its session record has ended.
    Assistant(Box<AssistantContainer>, bool),
}

impl Resolved {
    fn binding(&self, container: &ContainerRef) -> Result<Option<AgentBinding>> {
        match self {
            Resolved::Task(task) => Ok(task.agent_head(container)?.map(|head| head.binding)),
            Resolved::Pm(pm) => Ok(Some(pm.state.binding.clone())),
            Resolved::Assistant(assistant, _) => Ok(Some(assistant.state.binding.clone())),
        }
    }

    pub(crate) fn generation(
        &self,
        container: &ContainerRef,
        n: u32,
    ) -> Result<Option<AgentGeneration>> {
        match self {
            Resolved::Task(task) => task.agent_generation(container, n),
            Resolved::Pm(pm) => Ok(pm.state.generation(n).cloned()),
            Resolved::Assistant(assistant, _) => Ok(assistant.state.generation(n).cloned()),
        }
    }

    fn receipt(&self, container: &ContainerRef, id: &str) -> Result<Option<RequestReceipt>> {
        match self {
            Resolved::Task(task) => task.agent_receipt(container, id),
            Resolved::Pm(pm) => Ok(pm.state.receipt(id).cloned()),
            Resolved::Assistant(assistant, _) => Ok(assistant.state.receipt(id).cloned()),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Admitted {
    pub origin: RequestOrigin,
    pub resolved: Resolved,
}

#[derive(Debug)]
pub(crate) enum Admission {
    Replay(Box<RequestReceipt>),
    New(Box<Admitted>),
}

fn reject(code: RejectCode, reason: impl Into<String>) -> anyhow::Error {
    Rejection::new(code, reason).into()
}

pub(crate) fn container_of(identity: &RequestIdentity) -> ContainerRef {
    let task = identity.task.clone().unwrap_or_default();
    let run = identity.run.clone().unwrap_or_default();
    match (identity.role, &identity.review) {
        (AgentRole::Pm, _) => ContainerRef::Pm,
        (AgentRole::Assistant, _) => ContainerRef::Assistant {
            session: identity.assistant.clone().unwrap_or_default(),
        },
        (AgentRole::Reviewer, Some(review)) => ContainerRef::Reviewer {
            task,
            run,
            review: review.clone(),
        },
        (AgentRole::Planner, _) => ContainerRef::Planner { task, run },
        (AgentRole::PlanReviewer, Some(review)) => ContainerRef::PlanReviewer {
            task,
            plan: run,
            review: review.clone(),
        },
        _ => ContainerRef::Worker { task, run },
    }
}

fn resolve(store: &Store, container: &ContainerRef, envelope: &Envelope) -> Result<Resolved> {
    if let Some(id) = container.assistant() {
        if !store.assistant_exists(id)? {
            return Err(reject(
                RejectCode::ContainerNotFound,
                format!("assistant session {id} not found"),
            ));
        }
        let session = store.get_assistant(id)?;
        let ended = session.ended.is_some();
        let state = store.get_assistant_container(id)?;
        if state.state.binding.project != session.project.as_deref().unwrap_or("") {
            return Err(reject(
                RejectCode::ForeignProject,
                "the assistant's recorded project differs from its binding",
            ));
        }
        return Ok(Resolved::Assistant(Box::new(state), ended));
    }
    let Some(id) = container.task() else {
        let mut selection = pm_state::Selection::current()
            .key(
                "generations",
                format!("gen:{}", envelope.identity.generation),
            )
            .key("receipts", format!("request:{}", envelope.request_id));
        if envelope.op == "receipt_lookup"
            && let Some(request) = envelope.payload["request_id"].as_str()
        {
            selection = selection.key("receipts", format!("request:{request}"));
        }
        return pm_state::load_selected(store, &selection)?
            .map(|pm| Resolved::Pm(Box::new(pm)))
            .ok_or_else(|| reject(RejectCode::ContainerNotFound, "no corral-pi PM is bound"));
    };
    let task = match store.head(id) {
        Ok(task) => task,
        Err(error) if format!("{error}").ends_with("not found") => {
            return Err(reject(
                RejectCode::ContainerNotFound,
                format!("task {id} not found"),
            ));
        }
        Err(error) => return Err(error),
    };
    if task.agent_head(container)?.is_none() {
        return Err(reject(
            RejectCode::ContainerNotFound,
            format!("{} has no corral-pi binding", container.key()),
        ));
    }
    Ok(Resolved::Task(Box::new(task)))
}

/// Digest comparison without an early exit on the first differing byte.
fn digest_eq(secret: &str, expected: &str) -> bool {
    let actual = paths::sha256_hex(secret.as_bytes());
    actual.len() == expected.len()
        && actual
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn authenticate(
    store: &Store,
    envelope: &Envelope,
    binding: &AgentBinding,
    generation: Option<&AgentGeneration>,
    mode: Mode,
) -> Result<()> {
    let identity = &envelope.identity;
    if identity.store != paths::store_id(store) {
        return Err(reject(
            RejectCode::ForeignStore,
            "the envelope names another store",
        ));
    }
    if identity.project != binding.project {
        return Err(reject(
            RejectCode::ForeignProject,
            "the envelope names another project",
        ));
    }
    if identity.role != binding.role {
        return Err(reject(
            RejectCode::AuthFailed,
            "role does not match the container binding",
        ));
    }
    if identity.callback != binding.callback {
        return Err(reject(
            RejectCode::AuthFailed,
            "callback token does not match the binding",
        ));
    }
    let n = identity.generation;
    let generation = generation
        .ok_or_else(|| reject(RejectCode::AuthFailed, format!("generation {n} is unknown")))?;
    let nonce_matches = || {
        envelope
            .auth
            .bind_nonce
            .as_deref()
            .is_some_and(|nonce| digest_eq(nonce, &generation.bind_nonce_sha256))
    };
    match (&generation.bound, mode) {
        (None, Mode::Hello) if nonce_matches() => Ok(()),
        (None, Mode::Hello) => Err(reject(RejectCode::AuthFailed, "bind nonce does not match")),
        (None, _) => Err(reject(
            RejectCode::AuthFailed,
            format!("generation {n} has not completed hello"),
        )),
        (Some(bound), _) if !digest_eq(&envelope.auth.host_cap, &bound.host_cap_sha256) => {
            if mode == Mode::Hello && nonce_matches() {
                Err(reject(
                    RejectCode::NonceConsumed,
                    "the bind nonce was already consumed",
                ))
            } else {
                Err(reject(
                    RejectCode::AuthFailed,
                    format!("host_cap does not match generation {n}"),
                ))
            }
        }
        (Some(bound), _) if bound.session_id != identity.session_id => Err(reject(
            RejectCode::AuthFailed,
            format!("session does not match generation {n}"),
        )),
        (Some(_), _) => Ok(()),
    }
}

pub(crate) fn same_request(stored: &RequestReceipt, op: &str, payload_sha256: &str) -> Result<()> {
    if stored.op == op && stored.payload_sha256 == payload_sha256 {
        Ok(())
    } else {
        Err(reject(
            RejectCode::PayloadMismatch,
            "request_id was already used with a different op or payload",
        ))
    }
}

fn saved_pane(state: &AgentRunState, generation: u32, saved: Option<&str>) -> Result<()> {
    let launched = state.generation(generation).map(|g| g.pane.as_str());
    if launched.is_some() && launched == saved {
        Ok(())
    } else {
        Err(reject(
            RejectCode::PaneMismatch,
            "the generation's pane is not the container's saved pane",
        ))
    }
}

/// Liveness, fence and saved pane, repeated by every final transaction under the task lock.
pub(crate) fn recheck_task(task: &Task, binding: &RequestBinding) -> Result<()> {
    agent::check_live(task, binding)?;
    let state = agent::container_state(task, &binding.container)
        .context("the container lost its agent binding")?;
    let pane = agent::container_pane(task, &binding.container).map(|h| h.pane_id.as_str());
    saved_pane(state, binding.generation, pane)
}

/// Head counterpart of the final liveness and saved-pane fence.
pub(crate) fn recheck_task_head<T: corral_core::TaskRef + ?Sized>(
    task: &T,
    binding: &RequestBinding,
) -> Result<()> {
    agent::check_live_ref(task, binding)??;
    let saved = match &binding.container {
        ContainerRef::Worker { run, .. } => task.run_head(run)?.and_then(|r| r.herdr),
        ContainerRef::Reviewer { run, review, .. } => {
            task.review_child(run, review)?.and_then(|r| r.herdr)
        }
        ContainerRef::Planner { run, .. } => task.plan_run_head(run)?.and_then(|r| r.herdr),
        ContainerRef::PlanReviewer { plan, review, .. } => {
            task.plan_review(plan, review)?.and_then(|r| r.herdr)
        }
        ContainerRef::Pm | ContainerRef::Assistant { .. } => None,
    }
    .map(|h| h.pane_id);
    let launched = task
        .agent_generation(&binding.container, binding.generation)?
        .map(|g| g.pane);
    if launched.is_some() && launched == saved {
        Ok(())
    } else {
        Err(reject(
            RejectCode::PaneMismatch,
            "the generation's pane is not the container's saved pane",
        ))
    }
}

pub(crate) fn recheck_pm(pm: &AgentPm, generation: u32) -> Result<()> {
    agent::check_pm_live(pm, generation)?;
    saved_pane(&pm.state, generation, Some(&pm.pane))
}

pub(crate) fn recheck_assistant(
    assistant: &AssistantContainer,
    ended: bool,
    generation: u32,
) -> Result<()> {
    agent::check_assistant_live(&assistant.state, ended, generation)?;
    saved_pane(&assistant.state, generation, assistant.pane.as_deref())
}

pub(crate) fn admit(ctx: &Ctx, envelope: &Envelope, mode: Mode) -> Result<Admission> {
    let identity = &envelope.identity;
    let container = container_of(identity);
    let resolved = resolve(&ctx.store, &container, envelope)?;
    let bound = resolved
        .binding(&container)?
        .context("resolved container has no agent state")?;
    let generation = resolved.generation(&container, identity.generation)?;
    authenticate(&ctx.store, envelope, &bound, generation.as_ref(), mode)?;
    if let Some(stored) = resolved.receipt(&container, &envelope.request_id)? {
        same_request(&stored, &envelope.op, &envelope.payload_sha256)?;
        return Ok(Admission::Replay(Box::new(stored)));
    }
    if mode == Mode::Hello && generation.as_ref().is_some_and(|g| g.bound.is_some()) {
        return Err(reject(
            RejectCode::NonceConsumed,
            "the bind nonce was already consumed",
        ));
    }
    let binding = RequestBinding {
        container,
        generation: identity.generation,
        callback: identity.callback.clone(),
        settles_turn: match mode {
            Mode::Mutation { settles_turn } => settles_turn,
            Mode::Hello | Mode::Lookup => None,
        },
    };
    if mode != Mode::Lookup {
        match &resolved {
            Resolved::Task(task) => recheck_task_head(task.as_ref(), &binding)?,
            Resolved::Pm(pm) => recheck_pm(pm, identity.generation)?,
            Resolved::Assistant(assistant, ended) => {
                recheck_assistant(assistant, *ended, identity.generation)?
            }
        }
    }
    Ok(Admission::New(Box::new(Admitted {
        origin: RequestOrigin {
            request_id: envelope.request_id.clone(),
            op: envelope.op.clone(),
            payload_sha256: envelope.payload_sha256.clone(),
            identity: identity.clone(),
            binding,
        },
        resolved,
    })))
}

/// What a launcher binds: never chosen by the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindTarget {
    Planner {
        task: String,
        run: String,
    },
    PlanReviewer {
        task: String,
        plan: String,
        review: String,
    },
    Worker {
        task: String,
        run: String,
    },
    Reviewer {
        task: String,
        run: String,
        review: String,
        callback: String,
    },
    Pm {
        project: String,
    },
    Assistant {
        session: String,
    },
}

impl BindTarget {
    pub fn container(&self) -> ContainerRef {
        match self {
            BindTarget::Planner { task, run } => ContainerRef::Planner {
                task: task.clone(),
                run: run.clone(),
            },
            BindTarget::PlanReviewer { task, plan, review } => ContainerRef::PlanReviewer {
                task: task.clone(),
                plan: plan.clone(),
                review: review.clone(),
            },
            BindTarget::Worker { task, run } => ContainerRef::Worker {
                task: task.clone(),
                run: run.clone(),
            },
            BindTarget::Reviewer {
                task, run, review, ..
            } => ContainerRef::Reviewer {
                task: task.clone(),
                run: run.clone(),
                review: review.clone(),
            },
            BindTarget::Pm { .. } => ContainerRef::Pm,
            BindTarget::Assistant { session } => ContainerRef::Assistant {
                session: session.clone(),
            },
        }
    }
}

/// A new generation and its one-shot bind nonce, returned once for the bootstrap file.
#[derive(Clone)]
pub struct GenerationGrant {
    pub container: ContainerRef,
    pub generation: u32,
    pub bind_nonce: String,
    /// The snapshot this generation recorded; the bootstrap carries the same value.
    pub capabilities: CapabilitySnapshot,
}

impl std::fmt::Debug for GenerationGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenerationGrant")
            .field("container", &self.container)
            .field("generation", &self.generation)
            .field("bind_nonce", &"<redacted>")
            .finish()
    }
}

fn random_secret() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("generating a bind nonce: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn next_generation(
    state: &mut AgentRunState,
    nonce_digest: &str,
    pane: &str,
    capabilities: &CapabilitySnapshot,
    connection: Option<&corral_core::connection::ConnectionBinding>,
    fresh_session: bool,
) -> Result<u32> {
    if let Some(previous) = state
        .generations
        .iter()
        .filter(|g| g.bound.is_some())
        .max_by_key(|g| g.n)
    {
        corral_core::connection::ConnectionBinding::continue_from(
            previous.connection.as_ref(),
            connection,
            fresh_session,
        )?;
    }
    ensure!(
        state.generations.len() < GENERATIONS_MAX,
        "{} already opened {GENERATIONS_MAX} generations",
        state.binding.role.as_str()
    );
    let now = corral_core::clock::now();
    let n = state.generations.iter().map(|g| g.n).max().unwrap_or(0) + 1;
    let role = state.binding.role;
    let transition = state
        .generations
        .iter()
        .max_by_key(|g| g.n)
        .and_then(|previous| {
            capabilities.transition_from(previous.n, &previous.effective_capabilities(role))
        });
    for older in state.generations.iter_mut().filter(|g| g.fenced.is_none()) {
        older.fenced = Some(GenerationFence {
            reason: format!("replaced by generation {n}"),
            at: now,
        });
    }
    state.generations.push(AgentGeneration {
        n,
        bind_nonce_sha256: nonce_digest.into(),
        pane: pane.into(),
        created: now,
        bound: None,
        fenced: None,
        nudged_at: None,
        event_seq: 0,
        todo: capabilities.enabled("todo"),
        capabilities: Some(capabilities.clone()),
        capability_transition: transition,
        assets_sha256: None,
        connection: connection.cloned(),
    });
    state.binding.generation = n;
    state.binding.health = HostHealth::Unbound;
    state.binding.closing = false;
    Ok(n)
}

fn binding(role: AgentRole, store: &str, project: &str, callback: Option<String>) -> AgentBinding {
    AgentBinding {
        role,
        store: store.into(),
        project: project.into(),
        generation: 0,
        health: HostHealth::Unbound,
        callback,
        closing: false,
    }
}

/// Opens generation n+1 of a container in `pane`, fencing every older generation in the same
/// commit. Dispatch, review-run, pm open and resume call this before writing the bootstrap.
pub fn open_generation(store: &Store, target: &BindTarget, pane: &str) -> Result<GenerationGrant> {
    open_generation_with(store, target, pane, false, None)
}

impl BindTarget {
    pub fn role(&self) -> AgentRole {
        match self {
            BindTarget::Worker { .. } => AgentRole::Worker,
            BindTarget::Reviewer { .. } => AgentRole::Reviewer,
            BindTarget::Planner { .. } => AgentRole::Planner,
            BindTarget::PlanReviewer { .. } => AgentRole::PlanReviewer,
            BindTarget::Pm { .. } => AgentRole::Pm,
            BindTarget::Assistant { .. } => AgentRole::Assistant,
        }
    }
}

/// `open_generation`; with `adopt`, a PM's new pane is recorded as the owner's pane adopted by
/// `corral pi` in the same commit that moves the PM there, so a refused launch changes nothing.
pub fn open_generation_with(
    store: &Store,
    target: &BindTarget,
    pane: &str,
    adopt: bool,
    supported: Option<&BTreeSet<String>>,
) -> Result<GenerationGrant> {
    open_generation_bound(store, target, pane, adopt, supported, None, false)
}

pub(crate) fn open_generation_bound(
    store: &Store,
    target: &BindTarget,
    pane: &str,
    adopt: bool,
    supported: Option<&BTreeSet<String>>,
    connection: Option<&corral_core::connection::ConnectionBinding>,
    fresh_session: bool,
) -> Result<GenerationGrant> {
    if let Some(binding) = connection {
        binding.validate()?;
        binding.connection.require_admission()?;
    }
    // Settings resolve once per generation; an explicit request the host lacks refuses
    // before any record changes, while an unavailable default is only reported. The
    // project's text trust is recorded with them.
    let project = match (target, target.container().task()) {
        (BindTarget::Pm { project }, _) => (!project.is_empty()).then(|| project.clone()),
        (BindTarget::Assistant { session }, _) => store.get_assistant(session)?.project,
        (_, Some(task)) => store.head(task)?.project.clone(),
        (_, None) => None,
    };
    let mut capabilities =
        store
            .config
            .agent
            .capabilities
            .resolve_for(target.role(), project.as_deref(), supported);
    if matches!(target, BindTarget::Pm { project } if project.is_empty()) {
        // The workspace's own guidance is owner-controlled, not a project's text.
        // Rebuild so this trust decision is included in the durable snapshot digest.
        capabilities = corral_core::agent::CapabilitySnapshot::with_settings(
            capabilities.role,
            capabilities.preset,
            capabilities.features,
            capabilities.web,
            capabilities.mcp,
            Some(true),
            capabilities.llama_cpp,
        );
    }
    if let Some((key, reason)) = capabilities.explicit_unavailable() {
        bail!(
            "capability_unavailable: [agent.capabilities.{}] {key} = true, but {reason}, or \
             remove the override",
            target.role().as_str()
        );
    }
    // A disabled native planning role refuses here, before any record changes; corral never
    // falls back to another harness in its place.
    if let Some(state) = capabilities.features.get("role_enabled")
        && !state.enabled
    {
        let role = target.role().as_str();
        match &state.unavailable {
            Some(reason) => bail!("role_unavailable: native {role} role: {reason}"),
            None => bail!(
                "role_disabled: the native {role} role is off (preset {} or \
                 [agent.capabilities.{role}] role_enabled = false); enable it, or choose a \
                 non-native planning profile explicitly",
                capabilities.preset.as_str()
            ),
        }
    }
    let nonce = random_secret()?;
    let digest = paths::sha256_hex(nonce.as_bytes());
    let store_id = paths::store_id(store);
    let container = target.container();
    let generation = match (target, container.task()) {
        (BindTarget::Pm { project }, _) => pm_state::update_selected(
            store,
            &pm_state::Selection::default().family("generations"),
            |pm| {
                let pm = pm.get_or_insert_with(|| {
                    let state =
                        AgentRunState::new(binding(AgentRole::Pm, &store_id, project, None));
                    AgentPm::new(pane.into(), state)
                });
                // Adoption moves only with an adopting launch; another pane drops it, the same keeps it.
                if adopt {
                    pm.adopted_pane = Some(pane.into());
                } else if pm.pane != pane {
                    pm.adopted_pane = None;
                }
                pm.pane = pane.into();
                if pm.state.binding.project != *project {
                    pm.state.todo = None;
                }
                pm.state.binding.project = project.clone();
                Ok((
                    next_generation(
                        &mut pm.state,
                        &digest,
                        pane,
                        &capabilities,
                        connection,
                        fresh_session,
                    )?,
                    true,
                ))
            },
        )?,
        // The owner's pane moves with the generation, so a refused launch changes nothing.
        (BindTarget::Assistant { session }, _) => {
            store.update_assistant_container(session, |assistant| {
                assistant.pane = Some(pane.into());
                assistant.state.binding.project = project.clone().unwrap_or_default();
                next_generation(
                    &mut assistant.state,
                    &digest,
                    pane,
                    &capabilities,
                    connection,
                    fresh_session,
                )
            })?
        }
        (_, Some(task_id)) => {
            let (role, callback) = match target {
                BindTarget::Planner { .. } => (AgentRole::Planner, None),
                BindTarget::PlanReviewer { .. } => (AgentRole::PlanReviewer, None),
                BindTarget::Reviewer { callback, .. } => {
                    (AgentRole::Reviewer, Some(callback.clone()))
                }
                _ => (AgentRole::Worker, None),
            };
            let mut generation = 0;
            // T-626 also owns generation live counts, max identity and ordered fencing.
            // Attribute this retained-vector mutation, not its bounded caller/preflight.
            let known = corral_core::work::followup("T-626", "native_generation_open");
            store.update(task_id, |task| {
                let project = task
                    .project
                    .clone()
                    .context("a corral-pi container needs a task project")?;
                if let ContainerRef::Worker { run, .. } = &container
                    && task.current_run().map(|r| r.id.as_str()) != Some(run.as_str())
                {
                    bail!("only the current run can bind a corral-pi worker");
                }
                if matches!(
                    container,
                    ContainerRef::Planner { .. } | ContainerRef::PlanReviewer { .. }
                ) {
                    ensure!(
                        agent::container_closure(task, &container).is_none(),
                        "only a current open planning container can bind a corral-pi host"
                    );
                }
                let host = agent::container_host_mut(task, &container)
                    .with_context(|| format!("{} does not exist", container.key()))?;
                let state = host.get_or_insert_with(|| {
                    Box::new(AgentRunState::new(binding(
                        role,
                        &store_id,
                        &project,
                        callback.clone(),
                    )))
                });
                ensure!(
                    state.binding.role == role && state.binding.callback == callback,
                    "{} is bound to another role or review attempt",
                    container.key()
                );
                generation = next_generation(
                    state,
                    &digest,
                    pane,
                    &capabilities,
                    connection,
                    fresh_session,
                )?;
                Ok(())
            })?;
            drop(known);
            generation
        }
        (_, None) => bail!("a worker or reviewer target names a task"),
    };
    Ok(GenerationGrant {
        container,
        generation,
        bind_nonce: nonce,
        capabilities,
    })
}

fn fence(state: &mut AgentRunState, generation: u32, reason: &str) -> Result<bool> {
    let current = state.binding.generation == generation;
    let entry = state
        .generation_mut(generation)
        .with_context(|| format!("generation {generation} is unknown"))?;
    if entry.fenced.is_some() {
        return Ok(false);
    }
    entry.fenced = Some(GenerationFence {
        reason: reason.into(),
        at: corral_core::clock::now(),
    });
    if current {
        state.binding.health = HostHealth::Fenced;
    }
    Ok(true)
}

/// Permanently fences one generation; false when it already was. Its receipts stay readable
/// through lookups, and every later mutation it sends is refused `agent_fenced`.
pub fn fence_generation(
    store: &Store,
    container: &ContainerRef,
    generation: u32,
    reason: &str,
) -> Result<bool> {
    let detail = json!({"container": container.key(), "generation": generation, "reason": reason});
    match (container.assistant(), container.task()) {
        (Some(id), _) => {
            let changed = store.update_assistant_container(id, |assistant| {
                fence(&mut assistant.state, generation, reason)
            })?;
            if changed {
                store.log_event(None, "agent_fenced", detail)?;
            }
            Ok(changed)
        }
        (None, None) => {
            let changed = pm_state::update_selected(
                store,
                &pm_state::Selection::default().key("generations", format!("gen:{generation}")),
                |pm| {
                    let pm = pm.as_mut().context("no corral-pi PM is bound")?;
                    let changed = fence(&mut pm.state, generation, reason)?;
                    Ok((changed, changed))
                },
            )?;
            if changed {
                store.log_event(None, "agent_fenced", detail)?;
            }
            Ok(changed)
        }
        (None, Some(task_id)) => {
            let changed = store.try_update_head(task_id, |task| {
                let host = task
                    .agent_head(container)?
                    .with_context(|| format!("{} has no corral-pi binding", container.key()))?;
                let mut entry = task
                    .agent_generation(container, generation)?
                    .with_context(|| format!("generation {generation} is unknown"))?;
                if entry.fenced.is_some() {
                    return Ok(false);
                }
                entry.fenced = Some(GenerationFence {
                    reason: reason.into(),
                    at: store.now(),
                });
                task.put_agent_record(container, agent::NativeRecord::Generation(Box::new(entry)))?;
                if host.binding.generation == generation {
                    task.edit_agent_head(container, |host| {
                        host.binding.health = HostHealth::Fenced;
                        Ok(())
                    })?;
                }
                store.log_event(Some(task_id), "agent_fenced", detail)?;
                Ok(true)
            })?;
            Ok(changed.is_some())
        }
    }
}
