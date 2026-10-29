//! Admission in contract order: bounds (envelope.rs), resolution of the NAMED container,
//! authentication against the named generation, receipt lookup, then role liveness, current
//! generation and saved pane for new mutations. Also the generation binding primitives.
use anyhow::{Context, Result, bail, ensure};
use corral_core::agent::{
    self, AgentBinding, AgentGeneration, AgentPm, AgentRole, AgentRunState, ContainerRef,
    GENERATIONS_MAX, GenerationFence, HostHealth, RequestBinding, RequestIdentity, RequestOrigin,
    RequestReceipt, paths, pm_state,
};
use corral_core::{Store, Task};
use jiff::Timestamp;
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
    Task(Box<Task>),
    Pm(Box<AgentPm>),
}

impl Resolved {
    pub(crate) fn state(&self, container: &ContainerRef) -> Option<&AgentRunState> {
        match self {
            Resolved::Task(task) => agent::container_state(task, container),
            Resolved::Pm(pm) => Some(&pm.state),
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

fn resolve(store: &Store, container: &ContainerRef) -> Result<Resolved> {
    let Some(id) = container.task() else {
        return pm_state::load(store)?
            .map(|pm| Resolved::Pm(Box::new(pm)))
            .ok_or_else(|| reject(RejectCode::ContainerNotFound, "no Pi-native PM is bound"));
    };
    let task = match store.get(id) {
        Ok(task) => task,
        Err(error) if format!("{error}").ends_with("not found") => {
            return Err(reject(
                RejectCode::ContainerNotFound,
                format!("task {id} not found"),
            ));
        }
        Err(error) => return Err(error),
    };
    if agent::container_state(&task, container).is_none() {
        return Err(reject(
            RejectCode::ContainerNotFound,
            format!("{} has no Pi-native binding", container.key()),
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
    state: &AgentRunState,
    mode: Mode,
) -> Result<()> {
    let identity = &envelope.identity;
    let binding = &state.binding;
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
    let generation = state
        .generation(n)
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

pub(crate) fn recheck_pm(pm: &AgentPm, generation: u32) -> Result<()> {
    agent::check_pm_live(pm, generation)?;
    saved_pane(&pm.state, generation, Some(&pm.pane))
}

pub(crate) fn admit(ctx: &Ctx, envelope: &Envelope, mode: Mode) -> Result<Admission> {
    let identity = &envelope.identity;
    let container = container_of(identity);
    let resolved = resolve(&ctx.store, &container)?;
    let state = resolved
        .state(&container)
        .context("resolved container has no agent state")?;
    authenticate(&ctx.store, envelope, state, mode)?;
    if let Some(stored) = state.receipt(&envelope.request_id) {
        same_request(stored, &envelope.op, &envelope.payload_sha256)?;
        return Ok(Admission::Replay(Box::new(stored.clone())));
    }
    let generation = state.generation(identity.generation);
    if mode == Mode::Hello && generation.is_some_and(|g| g.bound.is_some()) {
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
            Resolved::Task(task) => recheck_task(task, &binding)?,
            Resolved::Pm(pm) => recheck_pm(pm, identity.generation)?,
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
        }
    }
}

/// A new generation and its one-shot bind nonce, returned once for the bootstrap file.
#[derive(Clone)]
pub struct GenerationGrant {
    pub container: ContainerRef,
    pub generation: u32,
    pub bind_nonce: String,
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
    capabilities: &corral_core::config::agent_capabilities::AgentCapabilities,
) -> Result<u32> {
    ensure!(
        state.generations.len() < GENERATIONS_MAX,
        "{} already opened {GENERATIONS_MAX} generations",
        state.binding.role.as_str()
    );
    let now = Timestamp::now();
    let n = state.generations.iter().map(|g| g.n).max().unwrap_or(0) + 1;
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
        todo: capabilities.todo(state.binding.role),
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
    open_generation_with(store, target, pane, false)
}

/// `open_generation`; with `adopt`, a PM's new pane is recorded as the owner's pane adopted by
/// `corral pi` in the same commit that moves the PM there, so a refused launch changes nothing.
pub fn open_generation_with(
    store: &Store,
    target: &BindTarget,
    pane: &str,
    adopt: bool,
) -> Result<GenerationGrant> {
    let nonce = random_secret()?;
    let digest = paths::sha256_hex(nonce.as_bytes());
    let store_id = paths::store_id(store);
    let container = target.container();
    let generation = match (target, container.task()) {
        (BindTarget::Pm { project }, _) => pm_state::update(store, |pm| {
            let pm = pm.get_or_insert_with(|| {
                let state = AgentRunState::new(binding(AgentRole::Pm, &store_id, project, None));
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
                    &store.config.agent.capabilities,
                )?,
                true,
            ))
        })?,
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
            store.update(task_id, |task| {
                let project = task
                    .project
                    .clone()
                    .context("a Pi-native container needs a task project")?;
                if let ContainerRef::Worker { run, .. } = &container
                    && task.current_run().map(|r| r.id.as_str()) != Some(run.as_str())
                {
                    bail!("only the current run can bind a Pi-native worker");
                }
                if matches!(
                    container,
                    ContainerRef::Planner { .. } | ContainerRef::PlanReviewer { .. }
                ) {
                    ensure!(
                        agent::container_closure(task, &container).is_none(),
                        "only a current open planning container can bind a Pi-native host"
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
                generation =
                    next_generation(state, &digest, pane, &store.config.agent.capabilities)?;
                Ok(())
            })?;
            generation
        }
        (_, None) => bail!("a worker or reviewer target names a task"),
    };
    Ok(GenerationGrant {
        container,
        generation,
        bind_nonce: nonce,
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
        at: Timestamp::now(),
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
    let changed = match container.task() {
        None => pm_state::update(store, |pm| {
            let pm = pm.as_mut().context("no Pi-native PM is bound")?;
            let changed = fence(&mut pm.state, generation, reason)?;
            Ok((changed, changed))
        })?,
        Some(task_id) => {
            let mut changed = false;
            store.try_update(task_id, |task| {
                let state = agent::container_state_mut(task, container)
                    .with_context(|| format!("{} has no Pi-native binding", container.key()))?;
                changed = fence(state, generation, reason)?;
                Ok(changed)
            })?;
            changed
        }
    };
    if changed {
        store.log_event(
            container.task(),
            "agent_fenced",
            json!({"container": container.key(), "generation": generation, "reason": reason}),
        )?;
    }
    Ok(changed)
}
