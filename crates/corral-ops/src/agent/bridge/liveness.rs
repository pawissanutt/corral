//! Host liveness and reconnect reconciliation (T-252 recovery, F3). Any corral process re-reads
//! a container's binding, proves the host process hello recorded is still the one running
//! (pid and kernel start time), reconnects through the generation's `ctl.cap`, reads
//! `get_state` and continues `events_since` from the saved sequence. A loss is a binding fact
//! recorded once per generation as `agent_lost`; nothing is inferred from an idle pane.
use anyhow::Result;
use corral_core::Store;
use corral_core::agent::{self, AgentRunState, ContainerRef, Executor, HostHealth, pm_state};
use corral_core::inbox::Recipient;
use serde_json::{Value, json};

use super::{BridgeClient, BridgeError, events_since_recorded};
use crate::agent::effects::executor_gone;
use crate::agent::launch::{SOCKET_FILE, generation_dir};

/// At most this many `events_since` pages per probe; a longer backlog continues next pass.
const PAGES_MAX: usize = 16;

#[cfg(test)]
thread_local! {
    static PROBING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether sync probes hosts: always in shipped builds. Test fixtures bind hosts with no process
/// behind them, so a test thread probes only after `probe_hosts_in_tests` turns it on.
pub fn probing() -> bool {
    #[cfg(test)]
    return PROBING.with(std::cell::Cell::get);
    #[cfg(not(test))]
    true
}

#[cfg(test)]
pub(crate) fn probe_hosts_in_tests() {
    PROBING.with(|on| on.set(true));
}

/// Why a bound host counts as lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossReason {
    /// No live process has the pid and start time hello recorded.
    HostExited,
    /// The host lives but its generation's `ctl.cap` is gone: control is impossible.
    ControlCredentialMissing,
    ControlCredentialUnsafe,
    /// The host lives but its socket is gone.
    SocketMissing,
    /// The socket refused or failed the connection or `get_state`.
    HostUnreachable,
    /// The host answers for another generation or session.
    HostMismatch,
}

impl LossReason {
    pub fn as_str(self) -> &'static str {
        match self {
            LossReason::HostExited => "host_exited",
            LossReason::ControlCredentialMissing => "control_credential_missing",
            LossReason::ControlCredentialUnsafe => "control_credential_unsafe",
            LossReason::SocketMissing => "socket_missing",
            LossReason::HostUnreachable => "host_unreachable",
            LossReason::HostMismatch => "host_mismatch",
        }
    }
}

/// What one probe of a container's current generation found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// The bound host answered; `seq` is the last event sequence now reconciled.
    Healthy { generation: u32, seq: u64 },
    Lost {
        generation: u32,
        reason: LossReason,
        detail: String,
    },
    /// The host is busy or still acknowledging its matching hello: no conclusion this pass.
    Busy,
    /// Nothing to probe: the binding is not healthy, or its generation is unbound or fenced.
    Skip,
}

/// `effects::executor_gone`, where a process that exits while its `/proc` entry is read
/// (`ESRCH`) is gone rather than an error.
pub fn gone(executor: &Executor) -> Result<bool> {
    match executor_gone(executor) {
        Err(error) if vanished(&error) => Ok(true),
        other => other,
    }
}

fn vanished(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|io| io.raw_os_error() == Some(3))
}

/// The host process hello recorded for the container's current generation.
pub fn bound_host(state: &AgentRunState) -> Option<Executor> {
    let bound = state.current_generation()?.bound.as_ref()?;
    Some(Executor {
        pid: bound.pid,
        start_time: bound.start_time.clone(),
    })
}

fn refusal(error: &anyhow::Error) -> Option<&BridgeError> {
    error.downcast_ref::<BridgeError>()
}

/// Probes the current generation of a healthy binding. Only a verified fact makes it lost: the
/// recorded process gone, or the process alive with its control credential, socket or state
/// unreachable.
pub fn probe(
    store: &Store,
    recipient: &Recipient,
    container: &ContainerRef,
    state: &AgentRunState,
) -> Result<Probe> {
    let Some(generation) = state.current_generation() else {
        return Ok(Probe::Skip);
    };
    let healthy = state.binding.health == HostHealth::Healthy && generation.fenced.is_none();
    let Some(bound) = generation.bound.as_ref().filter(|_| healthy) else {
        return Ok(Probe::Skip);
    };
    let n = generation.n;
    let lost = |reason, detail: String| {
        Ok(Probe::Lost {
            generation: n,
            reason,
            detail,
        })
    };
    let host = Executor {
        pid: bound.pid,
        start_time: bound.start_time.clone(),
    };
    if gone(&host)? {
        let detail = format!(
            "host pid {} (start time {}) is no longer running",
            bound.pid, bound.start_time
        );
        return lost(LossReason::HostExited, detail);
    }
    let dir = generation_dir(store, recipient, n)?;
    let mut client = match BridgeClient::connect_dir(&dir, n) {
        Ok(client) => client,
        Err(error) => {
            let reason = match refusal(&error).map(|e| e.code.as_str()) {
                Some("control_credential_missing") => LossReason::ControlCredentialMissing,
                Some("control_credential_unsafe") => LossReason::ControlCredentialUnsafe,
                _ if !dir.join(SOCKET_FILE).exists() => LossReason::SocketMissing,
                _ => LossReason::HostUnreachable,
            };
            return lost(reason, format!("{error:#}"));
        }
    };
    let reply = match client.get_state() {
        Ok(reply) => reply,
        Err(error) if refusal(&error).is_some_and(|e| e.code == "busy") => return Ok(Probe::Busy),
        Err(error) => return lost(LossReason::HostUnreachable, format!("{error:#}")),
    };
    // Rust publishes agent_bound before the host consumes its hello receipt. The authenticated
    // live process may still name only its current session during that acknowledgment window.
    if reply["generation"] == json!(n)
        && reply["bound"] == false
        && reply["session"].get("bound_id").is_some_and(Value::is_null)
        && reply["session"]["current_id"].as_str() == Some(bound.session_id.as_str())
    {
        return Ok(Probe::Busy);
    }
    if !answers_for(&reply, n, &bound.session_id) {
        let detail = format!(
            "get_state names generation {} and session {}, not generation {n} and session {}",
            reply["generation"], reply["session"]["bound_id"], bound.session_id
        );
        return lost(LossReason::HostMismatch, detail);
    }
    let mut seq = generation.event_seq;
    for _ in 0..PAGES_MAX {
        let page = match events_since_recorded(&mut client, store, container, seq) {
            Ok(page) => page,
            Err(error) if refusal(&error).is_some_and(|e| e.code == "busy") => break,
            Err(error) => return lost(LossReason::HostUnreachable, format!("{error:#}")),
        };
        seq = seq.max(page.next);
        if !page.more {
            break;
        }
    }
    Ok(Probe::Healthy { generation: n, seq })
}

fn answers_for(state: &Value, generation: u32, session: &str) -> bool {
    state["generation"] == json!(generation) && state["session"]["bound_id"] == json!(session)
}

fn apply(state: &mut AgentRunState, probe: &Probe) -> bool {
    match probe {
        Probe::Lost { generation, .. } => {
            let current = state.binding.generation == *generation;
            if current && state.binding.health == HostHealth::Healthy {
                state.binding.health = HostHealth::Lost;
                return true;
            }
            false
        }
        Probe::Healthy { generation, seq } => match state.generation_mut(*generation) {
            Some(g) if *seq > g.event_seq => {
                g.event_seq = *seq;
                true
            }
            _ => false,
        },
        Probe::Busy | Probe::Skip => false,
    }
}

/// Persists a probe: a loss turns the still-healthy current generation `lost` and logs
/// `agent_lost` once; a healthy answer advances the saved event sequence. True when it wrote.
pub fn record(store: &Store, container: &ContainerRef, probe: &Probe) -> Result<bool> {
    let changed = match container.task() {
        Some(id) => store
            .try_update(id, |task| {
                Ok(agent::container_state_mut(task, container).is_some_and(|s| apply(s, probe)))
            })?
            .is_some(),
        None => pm_state::update(store, |pm| {
            let changed = pm.as_mut().is_some_and(|pm| apply(&mut pm.state, probe));
            Ok((changed, changed))
        })?,
    };
    if changed
        && let Probe::Lost {
            generation,
            reason,
            detail,
        } = probe
    {
        store.log_event(
            container.task(),
            "agent_lost",
            json!({
                "container": container.key(), "generation": generation,
                "reason": reason.as_str(), "detail": detail,
            }),
        )?;
    }
    Ok(changed)
}
