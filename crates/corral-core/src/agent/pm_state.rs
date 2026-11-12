//! AgentPm v1: the corral-pi PM's backend state area, outside every task.
//! Only one PM is bound at a time; its task and run are always null.
use anyhow::Result;
use serde::{Deserialize, Serialize};

pub use super::pm_records::{Retention, RetentionLimits, Selection};
use super::{AGENT_RECORD_VERSION, AgentRunState, RetiredGeneration};
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

/// Upgrade the former whole-document journal before serving hot readers. Observing opens
/// and owning-sync preparation call this; observational preflights must not probe or write
/// PM state. Dispatch and PM locks serialize the upgrade with launch and fencing.
pub fn migrate(store: &Store) -> Result<()> {
    use std::sync::atomic::Ordering;

    if store.pm_upgrade_checked.load(Ordering::Acquire) {
        return Ok(());
    }
    if store
        .record::<serde_json::Value>("agent-pm", "current")?
        .is_some_and(|doc| {
            // Legacy import preserves opaque records too. Opening unrelated workspace
            // data must not require a valid PM binding; typed PM reads still validate it.
            doc.get("layout").is_none() && serde_json::from_value::<AgentPm>(doc).is_ok()
        })
    {
        update(store, |_| Ok(((), true)))?;
    }
    // Current writers publish only layout-2 records. A later sync on this handle
    // needs no repeat probe; all live PM reads still go through the record facade.
    store.pm_upgrade_checked.store(true, Ordering::Release);
    Ok(())
}

/// Reads the area without a lock. Inside a task's `Store::update` this is the consistent read:
/// every AgentPm writer holds the dispatch lock that the task update already holds.
pub fn load(store: &Store) -> Result<Option<AgentPm>> {
    load_selected(store, &Selection::all())
}

/// Read-modify-write under the dispatch lock, so a PM fence and a task transaction that reads
/// the PM binding are serialized. `f` returns whether to write. Never call this inside a task
/// update (the dispatch lock is not reentrant).
pub fn update<R>(
    store: &Store,
    f: impl FnOnce(&mut Option<AgentPm>) -> Result<(R, bool)>,
) -> Result<R> {
    update_selected(store, &Selection::all(), f)
}

/// Reads only the head and explicitly selected children. Use `load` for history output.
pub fn load_selected(store: &Store, selection: &Selection) -> Result<Option<AgentPm>> {
    if store.has_record_unit() {
        // Preserve staged writes and the enclosing task/record transaction's pinned view.
        return Ok(super::pm_records::read(store, selection)?.map(|(_, pm)| pm));
    }
    let snapshot = store.read_only_snapshot()?;
    Ok(super::pm_records::read(&snapshot, selection)?.map(|(_, pm)| pm))
}

/// The durable family size, including children omitted from a selected read.
/// Call under the same transaction as a limit check and its append.
pub fn child_count(store: &Store, kind: &str) -> Result<u64> {
    super::pm_records::count(store, kind)
}

/// One retention pass over the PM container at `limits` (`RetentionLimits::PM` in shipped
/// code): evicts settled receipts of ended generations and retires ended generations once
/// either family nears its cap. Call under the same transaction as the limit check it precedes.
pub fn retain(store: &Store, limits: &RetentionLimits) -> Result<Retention> {
    super::pm_records::retain(store, limits)
}

/// The usage attribution of every retired PM generation, oldest first. History for usage
/// reporting only; read it after the PM state it completes, since records are only added.
pub fn retired_generations(store: &Store) -> Result<Vec<RetiredGeneration>> {
    if store.has_record_unit() {
        return super::pm_records::retired(store);
    }
    super::pm_records::retired(&store.read_only_snapshot()?)
}

/// The binding and current host generation, independent of retained history.
pub fn load_current(store: &Store) -> Result<Option<AgentPm>> {
    load_selected(store, &Selection::current())
}

/// A serialized patch of the selected children and scalar head. Unselected history is
/// preserved, including when the callback appends or removes a selected record.
pub fn update_selected<R>(
    store: &Store,
    selection: &Selection,
    f: impl FnOnce(&mut Option<AgentPm>) -> Result<(R, bool)>,
) -> Result<R> {
    update_inner(store, selection, None, f)
}

/// `update_selected` preceded by a retention pass at `limits` in the same transaction, so the
/// callback reads only what retention kept. Opening a generation uses it.
pub fn update_retained<R>(
    store: &Store,
    selection: &Selection,
    limits: &RetentionLimits,
    f: impl FnOnce(&mut Option<AgentPm>) -> Result<(R, bool)>,
) -> Result<R> {
    update_inner(store, selection, Some(limits), f)
}

fn update_inner<R>(
    store: &Store,
    selection: &Selection,
    retention: Option<&RetentionLimits>,
    f: impl FnOnce(&mut Option<AgentPm>) -> Result<(R, bool)>,
) -> Result<R> {
    let _board = store.lock("dispatch")?;
    let _pm = store.lock("agent-pm")?;
    store.records_transaction(|| {
        if let Some(limits) = retention {
            super::pm_records::retain(store, limits)?;
        }
        let before = super::pm_records::read(store, selection)?;
        let mut pm = before.as_ref().map(|(_, pm)| pm.clone());
        let (result, write) = f(&mut pm)?;
        if write {
            match pm {
                Some(pm) => super::pm_records::save(store, before.as_ref(), &pm)?,
                None => super::pm_records::delete(store)?,
            }
        }
        Ok(result)
    })
}
