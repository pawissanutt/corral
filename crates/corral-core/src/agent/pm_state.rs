//! AgentPm v1: the Pi-native PM's state area, outside every task (`.corral/state/agent-pm.json`).
//! Only one PM is bound at a time; its task and run are always null.
use std::fs;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{AGENT_RECORD_VERSION, AgentRunState};
use crate::Store;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentPm {
    pub version: u32,
    /// The saved PM pane; every generation must run in it.
    pub pane: String,
    /// Binding (role pm, project = the one authorized scope), generations, receipts and turns.
    pub state: AgentRunState,
}

impl AgentPm {
    pub fn new(pane: String, state: AgentRunState) -> Self {
        Self {
            version: AGENT_RECORD_VERSION,
            pane,
            state,
        }
    }
}

/// Reads the area without a lock. Inside a task's `Store::update` this is the consistent read:
/// every AgentPm writer holds the dispatch lock that the task update already holds.
pub fn load(store: &Store) -> Result<Option<AgentPm>> {
    let path = super::paths::pm_state_path(store);
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .with_context(|| format!("parsing {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// Read-modify-write under the dispatch lock, so a PM fence and a task transaction that reads
/// the PM binding are serialized. `f` returns whether to write. Never call this inside a task
/// update (the dispatch lock is not reentrant).
pub fn update<R>(
    store: &Store,
    f: impl FnOnce(&mut Option<AgentPm>) -> Result<(R, bool)>,
) -> Result<R> {
    let _board = store.lock("dispatch")?;
    let _pm = store.lock("agent-pm")?;
    let mut pm = load(store)?;
    let (result, write) = f(&mut pm)?;
    if write {
        let path = super::paths::pm_state_path(store);
        let parent = path.parent().context("agent-pm state path has no parent")?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        let text = serde_json::to_string_pretty(&pm).context("serializing agent-pm state")?;
        crate::store::atomic_write(&path, &text)?;
    }
    Ok(result)
}
