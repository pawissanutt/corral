//! Bounded PM cost evidence, independent of generation/receipt retention and never a limit.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{TurnOrigin, TurnOriginKind};
use crate::Store;

pub const PENDING_MAX: usize = 256;
pub const ATTRIBUTIONS_MAX: usize = 4096;
const AREA: &str = "agent-pm-cost";
const PENDING: &str = "agent-pm-cost-pending";
const BOUND: &str = "agent-pm-cost-bound";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostClass {
    Item,
    Shared,
    Overhead,
    #[default]
    Unallocated,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cause {
    pub class: CostClass,
    pub project: Option<String>,
    pub task: Option<String>,
    pub item_key: Option<String>,
    pub kinds: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PendingCause {
    /// The bridge wire key, not the inbox's `@env` spelling.
    pub envelope_key: String,
    pub cause: Cause,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CostAttribution {
    pub generation: u32,
    pub turn: u64,
    pub envelope_key: Option<String>,
    pub class: CostClass,
    pub project: Option<String>,
    pub task: Option<String>,
    pub item_key: Option<String>,
    pub kinds: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Index {
    pending: Vec<String>,
    bound: Vec<(u32, u64)>,
    dropped: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CostRecords {
    pub pending: Vec<PendingCause>,
    pub attributions: Vec<CostAttribution>,
    pub dropped: u64,
}

fn index(store: &Store) -> Result<Index> {
    Ok(store.record(AREA, "current")?.unwrap_or_default())
}

fn pending_key(key: &str) -> String {
    super::paths::sha256_hex(key.as_bytes())
}

fn bound_key(generation: u32, turn: u64) -> String {
    format!("{generation:010}:{turn:020}")
}

/// A replay does not add a row or move it to the back of the eviction queue.
/// All child writes and eviction metadata commit together, joining an existing transaction.
pub fn prepare(store: &Store, mut pending: PendingCause) -> Result<()> {
    pending.cause.kinds.sort();
    pending.cause.kinds.dedup();
    pending.cause.kinds.truncate(4);
    let _lock = store.lock("agent-pm-cost")?;
    store.records_transaction(|| {
        let mut head = index(store)?;
        let key = pending_key(&pending.envelope_key);
        if !head.pending.contains(&key) {
            head.pending.push(key.clone());
        }
        store.save_record(PENDING, &key, &pending)?;
        while head.pending.len() > PENDING_MAX {
            store.delete_record(PENDING, &head.pending.remove(0))?;
            head.dropped = head.dropped.saturating_add(1);
        }
        store.save_record(AREA, "current", &head)
    })
}

pub fn attribution(store: &Store, generation: u32, turn: u64) -> Result<Option<CostAttribution>> {
    store.record(BOUND, &bound_key(generation, turn))
}

/// `continued_from` is supplied only when the canonical request ledger links this origin key
/// to its granting turn. No previous-turn or time-based guess is made.
pub fn bind(
    store: &Store,
    generation: u32,
    turn: u64,
    origin: &TurnOrigin,
    continued_from: Option<(u32, u64)>,
) -> Result<CostAttribution> {
    let _lock = store.lock("agent-pm-cost")?;
    store.records_transaction(|| {
        if let Some(existing) = attribution(store, generation, turn)? {
            return Ok(existing);
        }
        let mut head = index(store)?;
        let mut result = CostAttribution {
            generation,
            turn,
            ..CostAttribution::default()
        };
        match origin.kind {
            TurnOriginKind::Bridge => {
                if let Some(wire) = origin.idempotency_key.as_deref() {
                    let key = pending_key(wire);
                    if let Some(pending) = store.record::<PendingCause>(PENDING, &key)? {
                        let cause = pending.cause;
                        result.envelope_key = Some(pending.envelope_key);
                        result.class = cause.class;
                        result.project = cause.project;
                        result.task = cause.task;
                        result.item_key = cause.item_key;
                        result.kinds = cause.kinds;
                        store.delete_record(PENDING, &key)?;
                        head.pending.retain(|old| *old != key);
                    }
                }
            }
            TurnOriginKind::Continuation => {
                if let Some((parent_generation, parent_turn)) = continued_from
                    && let Some(parent) = attribution(store, parent_generation, parent_turn)?
                {
                    result = CostAttribution {
                        generation,
                        turn,
                        ..parent
                    };
                }
            }
            _ => {}
        }
        result.kinds.truncate(4);
        store.save_record(BOUND, &bound_key(generation, turn), &result)?;
        head.bound.push((generation, turn));
        while head.bound.len() > ATTRIBUTIONS_MAX {
            let (generation, turn) = head.bound.remove(0);
            store.delete_record(BOUND, &bound_key(generation, turn))?;
            head.dropped = head.dropped.saturating_add(1);
        }
        store.save_record(AREA, "current", &head)?;
        Ok(result)
    })
}

/// Output-sized read. Generation retirement deliberately does not touch this area.
pub fn load(store: &Store) -> Result<CostRecords> {
    let head = index(store)?;
    let pending = head
        .pending
        .iter()
        .map(|key| {
            store
                .record(PENDING, key)?
                .context("dangling PM pending cost index")
        })
        .collect::<Result<Vec<_>>>()?;
    let attributions = head
        .bound
        .iter()
        .map(|(generation, turn)| {
            attribution(store, *generation, *turn)?.context("dangling PM cost attribution index")
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(CostRecords {
        pending,
        attributions,
        dropped: head.dropped,
    })
}

#[cfg(test)]
#[path = "pm_cost_tests.rs"]
mod tests;
