//! Versioned host-authenticated executor facts, separate from the legacy Activity wire shape.
//! Unknown evidence is never quiescence. No recovery input is used to signal a process.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use corral_core::{Store, assistant::AssistantContainer};
use serde::{Deserialize, Serialize};

pub(super) const AREA: &str = "assistant-effect-evidence";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_time: String,
    pub boot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Executor {
    Preparing {
        resource: String,
        config_sha256: String,
    },
    Process {
        resource: String,
        config_sha256: String,
        process: ProcessIdentity,
    },
    Http {
        resource: String,
        config_sha256: String,
        connection: String,
    },
}
impl Executor {
    fn scope(&self) -> (&str, &str) {
        match self {
            Self::Preparing {
                resource,
                config_sha256,
            }
            | Self::Process {
                resource,
                config_sha256,
                ..
            }
            | Self::Http {
                resource,
                config_sha256,
                ..
            } => (resource, config_sha256),
        }
    }
    fn validate(&self) -> Result<()> {
        let (resource, digest) = self.scope();
        ensure!(
            !resource.is_empty() && resource.len() <= 1024 && hash(digest),
            "invalid executor scope"
        );
        match self {
            Self::Process { process, .. } => ensure!(
                process.pid > 0
                    && !process.start_time.is_empty()
                    && process.start_time.len() <= 128
                    && !process.boot_id.is_empty()
                    && process.boot_id.len() <= 128,
                "missing owned process identity"
            ),
            Self::Http { connection, .. } => ensure!(
                !connection.is_empty() && connection.len() <= 128,
                "missing connection instance"
            ),
            Self::Preparing { .. } => {}
        }
        Ok(())
    }
}

/// Outcome describes earlier work, NOT whether an executor is now quiescent. An unknown
/// effect result is retained even when the owned local process has demonstrably terminated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    pub disposition: Disposition,
    pub executor_sha256: String,
    pub exit_observed: bool,
    pub group_clear: bool,
    pub requests_terminal: bool,
    pub outcome: String,
    pub delivery: String,
    pub requests: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Completed,
    ShutdownVerified,
    NotStarted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    pub executor: Executor,
    pub terminal: Option<Terminal>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub version: u32,
    pub store: String,
    pub project: String,
    pub assistant: String,
    pub generation: u32,
    pub session_id: String,
    pub admission: String,
    pub tool: String,
    pub instances: BTreeMap<String, Instance>,
    pub sealed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Lifecycle {
    Prepare {
        instance: String,
        executor: Executor,
    },
    Bind {
        instance: String,
        executor: Executor,
    },
    Terminal {
        instance: String,
        terminal: Terminal,
    },
    Seal,
}

pub(super) fn key(assistant: &str, generation: u32, admission: &str) -> String {
    format!("{assistant}/g{generation}/{}", digest(admission.as_bytes()))
}
pub(super) fn digest(bytes: &[u8]) -> String {
    corral_core::blob_ref::BlobHash::of(bytes).to_string()
}
pub(super) fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}
pub(super) fn executor_digest(executor: &Executor) -> Result<String> {
    Ok(digest(&serde_json::to_vec(executor)?))
}

impl Evidence {
    pub(super) fn validate_target(
        &self,
        store: &Store,
        assistant: &str,
        container: &AssistantContainer,
        generation: u32,
        admission: &super::boundary::Activity,
    ) -> Result<()> {
        let original = container
            .state
            .generations
            .iter()
            .find(|g| g.n == generation)
            .and_then(|g| g.bound.as_ref())
            .context("original bound session missing")?;
        ensure!(
            self.version == 1
                && self.store == container.state.binding.store
                && self.store == corral_core::agent::paths::store_id(store)
                && self.project == container.state.binding.project
                && self.assistant == assistant
                && self.generation == generation
                && admission.generation == generation
                && self.session_id == original.session_id
                && self.admission == admission.id
                && self.tool == admission.tool,
            "executor evidence version/source/generation/admission mismatch"
        );
        Ok(())
    }
    pub fn require_terminal(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && self.sealed
                && !self.instances.is_empty()
                && self.instances.len() <= 128,
            "executor evidence incomplete/unsealed"
        );
        for (token, instance) in &self.instances {
            ensure!(
                !token.is_empty() && token.len() <= 128,
                "invalid executor instance token"
            );
            instance.executor.validate()?;
            let terminal = instance
                .terminal
                .as_ref()
                .context("executor shutdown unknown")?;
            ensure!(
                terminal.executor_sha256 == executor_digest(&instance.executor)?
                    && terminal.outcome.len() <= 1024
                    && !terminal.outcome.is_empty()
                    && !terminal.delivery.is_empty()
                    && terminal.delivery.len() <= 128
                    && terminal.requests.len() <= 128
                    && terminal
                        .requests
                        .iter()
                        .all(|r| !r.is_empty() && r.len() <= 256),
                "terminal proof identity/result mismatch"
            );
            match (&instance.executor, &terminal.disposition) {
                (Executor::Preparing { .. }, Disposition::NotStarted) => ensure!(
                    terminal.requests.is_empty(),
                    "not-started has dispatched requests"
                ),
                (
                    Executor::Process { .. },
                    Disposition::Completed | Disposition::ShutdownVerified,
                ) => ensure!(
                    terminal.exit_observed && terminal.group_clear,
                    "owned process exit/group clearance unproved"
                ),
                (Executor::Http { .. }, Disposition::Completed | Disposition::ShutdownVerified) => {
                    ensure!(
                        terminal.requests_terminal,
                        "remote request outcome unknown; disconnect is not terminal proof"
                    )
                }
                _ => anyhow::bail!("terminal disposition does not match executor"),
            }
        }
        Ok(())
    }
}

/// Called only by the current authenticated host inside its receipted assistant unit.
/// The prepare fact precedes spawn; concrete identity precedes protocol dispatch.
pub(super) fn publish(
    store: &Store,
    assistant: &str,
    container: &AssistantContainer,
    generation: u32,
    id: &str,
    event: Lifecycle,
) -> Result<()> {
    let active = super::boundary::activities(store, assistant)?;
    let row = active
        .iter()
        .find(|r| r.generation == generation && r.id == id)
        .context("executor admission missing")?;
    let bound = container
        .state
        .generations
        .iter()
        .find(|g| g.n == generation)
        .and_then(|g| g.bound.as_ref())
        .context("host binding missing")?;
    let key = key(assistant, generation, id);
    let mut evidence: Evidence = store.record(AREA, &key)?.unwrap_or_else(|| Evidence {
        version: 1,
        store: container.state.binding.store.clone(),
        project: container.state.binding.project.clone(),
        assistant: assistant.into(),
        generation,
        session_id: bound.session_id.clone(),
        admission: id.into(),
        tool: row.tool.clone(),
        instances: BTreeMap::new(),
        sealed: false,
    });
    evidence.validate_target(store, assistant, container, generation, row)?;
    transition(&mut evidence, event)?;
    store.save_record(AREA, &key, &evidence)
}

/// Same closed state machine for canonical events and retained original-host journal facts.
pub(super) fn transition(evidence: &mut Evidence, event: Lifecycle) -> Result<()> {
    ensure!(
        !evidence.sealed,
        "executor evidence already sealed; use retained proof, never repeat shutdown"
    );
    match event {
        Lifecycle::Prepare { instance, executor } => {
            executor.validate()?;
            ensure!(
                matches!(executor, Executor::Preparing { .. })
                    && !instance.is_empty()
                    && instance.len() <= 128
                    && evidence.instances.len() < 128
                    && !evidence.instances.contains_key(&instance),
                "invalid/duplicate executor preparation"
            );
            evidence.instances.insert(
                instance,
                Instance {
                    executor,
                    terminal: None,
                },
            );
        }
        Lifecycle::Bind { instance, executor } => {
            executor.validate()?;
            let prior = evidence
                .instances
                .get_mut(&instance)
                .context("executor was not prepared")?;
            ensure!(
                matches!(prior.executor, Executor::Preparing { .. })
                    && !matches!(executor, Executor::Preparing { .. })
                    && prior.executor.scope() == executor.scope()
                    && prior.terminal.is_none(),
                "executor identity cannot be replaced"
            );
            prior.executor = executor;
        }
        Lifecycle::Terminal { instance, terminal } => {
            let prior = evidence
                .instances
                .get_mut(&instance)
                .context("unknown executor instance")?;
            ensure!(
                prior.terminal.as_ref().is_none_or(|old| *old == terminal),
                "terminal proof is immutable"
            );
            prior.terminal = Some(terminal);
            let mut single = evidence.clone();
            single.instances.retain(|token, _| token == &instance);
            single.sealed = true;
            single.require_terminal()?;
        }
        Lifecycle::Seal => {
            evidence.sealed = true;
            evidence.require_terminal()?;
        }
    }
    Ok(())
}
