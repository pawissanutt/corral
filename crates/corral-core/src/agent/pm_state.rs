//! AgentPm v1: the Pi-native PM's backend state area, outside every task.
//! Only one PM is bound at a time; its task and run are always null.
use anyhow::Result;
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
    /// The owner's own pane `corral pi` adopted. It is in effect only while it is the saved
    /// `pane`: corral never closes it, and types into it only after verifying its live host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopted_pane: Option<String>,
}

impl AgentPm {
    pub fn new(pane: String, state: AgentRunState) -> Self {
        Self {
            version: AGENT_RECORD_VERSION,
            pane,
            state,
            adopted_pane: None,
        }
    }

    /// True while the saved PM pane is the owner's pane adopted by `corral pi`.
    pub fn adopted(&self) -> bool {
        self.adopted_pane.as_deref() == Some(self.pane.as_str())
    }
}

/// Reads the area without a lock. Inside a task's `Store::update` this is the consistent read:
/// every AgentPm writer holds the dispatch lock that the task update already holds.
pub fn load(store: &Store) -> Result<Option<AgentPm>> {
    // T-625: PM-authorized known cost. Keep the canonical AgentPm document intact;
    // normalization is a separate persistent design, not part of T-337 integration.
    let _known = crate::work::followup("T-625", "root_pm_load");
    store.record("agent-pm", "current")
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
    store.records_transaction(|| {
        let mut pm = load(store)?;
        let (result, write) = f(&mut pm)?;
        if write {
            match pm {
                Some(pm) => {
                    let _known = crate::work::followup("T-625", "root_pm_save");
                    store.save_record("agent-pm", "current", &pm)?;
                }
                None => store.delete_record("agent-pm", "current")?,
            }
        }
        Ok(result)
    })
}
