//! Rebuildable board read model; no socket, background thread, runtime probe or history scan.
//!
//! Attention keys are the bounded ledger published by `attention` reconciliation. Missing
//! legacy facts remain unknown: EventSeq, runtime/machine, bare dispatch-hold reason and
//! progress publication time. Rail freshness compares recorded candidates, not live Git.
mod cache;
pub mod changes;
mod rail;
mod row;

pub use cache::cache_path;

#[cfg(test)]
mod projection_tests;

use anyhow::{Context, Result};
use corral_core::backend::{ChangeScope, ChangeSeq, FeedError, HeadQuery};
use corral_core::{Status, Store};
use corral_protocol::board::{BoardCursor, BoardDelta, BoardRow};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::Ctx;

pub const CACHE_FILE: &str = "boardview-v1.bin";

/// A snapshot and its durable change-feed checkpoint. Do not retain a store view here.
#[derive(Debug, Serialize, Deserialize)]
pub struct Projection {
    pub epoch: u64,
    pub folded_through: ChangeSeq,
    rows: BTreeMap<String, BoardRow>,
    left: BTreeMap<String, u64>,
    inputs: BTreeMap<String, BTreeSet<String>>,
    #[serde(skip)]
    dependents: BTreeMap<String, BTreeSet<String>>,
    #[serde(skip)]
    revisions: BTreeMap<u64, BTreeSet<String>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct FoldOutcome {
    /// A feed gap (or store replacement) caused a new epoch; every subscriber must reset.
    pub reset: bool,
    pub changed: Vec<String>,
    pub left: Vec<String>,
    pub folded_through: ChangeSeq,
}

/// A reset is explicit and contains the new epoch, not an ambiguous empty delta.
/// After Reset, discard local rows and request the new epoch with `BoardCursor::default()`.
#[derive(Debug, PartialEq)]
pub enum DeltaOutcome {
    Reset { epoch: u64 },
    Delta(BoardDelta),
}

/// Compute one open row on one pinned snapshot. Unknown/closed tasks return None.
pub fn compute_row(ctx: &Ctx, id: &str) -> Result<Option<BoardRow>> {
    let store = ctx
        .store
        .read_only_snapshot()
        .context("pinning board row snapshot")?;
    let seq = store.view().context("reading board row checkpoint")?.seq();
    let Some(head) = store
        .head_if_exists(id)
        .with_context(|| format!("reading board head {id}"))?
    else {
        return Ok(None);
    };
    if closed(head.status) {
        return Ok(None);
    }
    let mut row = row::compute(&store, &head)?.row;
    row.rev = seq.0;
    Ok(Some(row))
}

/// Compute all open rows without retaining a projection or reading closed-task history.
pub fn compute_rows(ctx: &Ctx) -> Result<Vec<BoardRow>> {
    let store = ctx
        .store
        .read_only_snapshot()
        .context("pinning board rows snapshot")?;
    let view = store.view().context("reading board rows checkpoint")?;
    let heads = store
        .heads_checked(&HeadQuery::Open { project: None })
        .context("reading open board rows")?;
    let rows = heads
        .iter()
        .map(|head| {
            let mut row = row::compute(&store, head)?.row;
            row.rev = view.seq().0;
            Ok(row)
        })
        .collect::<Result<_>>()?;
    view.ensure_fresh()
        .context("finishing board rows snapshot")?;
    Ok(rows)
}

impl Projection {
    pub fn rebuild(ctx: &Ctx) -> Result<Self> {
        let store = ctx
            .store
            .read_only_snapshot()
            .context("pinning board rebuild snapshot")?;
        Self::rebuild_store(&store)
    }

    fn rebuild_store(store: &Store) -> Result<Self> {
        let view = store.view().context("reading board rebuild checkpoint")?;
        let folded_through = view.seq();
        let heads = store
            .heads_checked(&HeadQuery::Open { project: None })
            .context("reading open board heads")?;
        let mut projection = Self {
            epoch: new_epoch()?,
            folded_through,
            rows: BTreeMap::new(),
            left: BTreeMap::new(),
            inputs: BTreeMap::new(),
            dependents: BTreeMap::new(),
            revisions: BTreeMap::new(),
        };
        for head in heads {
            let mut computed = row::compute(store, &head)?;
            computed.row.rev = folded_through.0;
            projection.replace_inputs(&head.id, computed.inputs);
            projection.index(folded_through.0, &head.id);
            projection.rows.insert(head.id.clone(), computed.row);
        }
        view.ensure_fresh()
            .context("finishing board rebuild snapshot")?;
        Ok(projection)
    }

    pub fn rows(&self) -> &BTreeMap<String, BoardRow> {
        &self.rows
    }

    /// Fold all retained pages on one snapshot. Work is proportional to feed entries and
    /// affected rows, including direct dependents indexed during prior computations.
    /// All fallible reads finish before the in-memory checkpoint/rows are changed.
    pub fn fold(&mut self, ctx: &Ctx) -> Result<FoldOutcome> {
        let store = ctx
            .store
            .read_only_snapshot()
            .context("pinning board fold snapshot")?;
        let view = store.view().context("opening board change feed")?;
        if view.seq() < self.folded_through {
            return self.reset(&store);
        }
        let mut cursor = self.folded_through;
        let mut affected = BTreeSet::new();
        loop {
            let page = match view.changes(cursor, 256) {
                Ok(page) => page,
                Err(FeedError::Gap { .. }) => return self.reset(&store),
                Err(error) => return Err(error).context("reading board change page"),
            };
            for entry in page.entries {
                for scope in entry.scopes {
                    affected_scope(&scope, &mut affected);
                }
                cursor = entry.seq;
            }
            if !page.more {
                break;
            }
        }
        // A member's goal or queue position can change when its named parent/prerequisite
        // changes, even if the member's own head was not written.
        let direct = affected;
        let mut affected = BTreeSet::new();
        for id in direct {
            if let Some(dependents) = self.dependents.get(&id) {
                affected.extend(dependents.iter().cloned());
            }
            affected.insert(id);
        }
        let mut computed = Vec::with_capacity(affected.len());
        for id in affected {
            let head = store
                .head_if_exists(&id)
                .with_context(|| format!("reading changed board head {id}"))?;
            let row = head
                .as_ref()
                .filter(|head| !closed(head.status))
                .map(|head| row::compute(&store, head))
                .transpose()?;
            computed.push((id, row));
        }
        view.ensure_fresh()
            .context("finishing board fold snapshot")?;
        let mut outcome = FoldOutcome {
            reset: false,
            changed: Vec::new(),
            left: Vec::new(),
            folded_through: cursor,
        };
        for (id, computed) in computed {
            match computed {
                Some(mut computed) => {
                    self.replace_inputs(&id, computed.inputs);
                    if let Some(previous) = self.rows.get(&id) {
                        computed.row.rev = previous.rev;
                        if previous == &computed.row {
                            continue;
                        }
                    }
                    self.unindex_id(&id);
                    computed.row.rev = cursor.0;
                    self.rows.insert(id.clone(), computed.row);
                    self.left.remove(&id);
                    self.index(cursor.0, &id);
                    outcome.changed.push(id);
                }
                None if self.rows.contains_key(&id) => {
                    self.unindex_id(&id);
                    self.rows.remove(&id);
                    self.replace_inputs(&id, BTreeSet::new());
                    self.inputs.remove(&id);
                    self.left.insert(id.clone(), cursor.0);
                    self.index(cursor.0, &id);
                    outcome.left.push(id);
                }
                None => {}
            }
        }
        self.folded_through = cursor;
        Ok(outcome)
    }

    fn reset(&mut self, store: &Store) -> Result<FoldOutcome> {
        let mut rebuilt = Self::rebuild_store(store)?;
        while rebuilt.epoch == self.epoch {
            rebuilt.epoch = new_epoch()?;
        }
        let outcome = FoldOutcome {
            reset: true,
            changed: rebuilt.rows.keys().cloned().collect(),
            left: Vec::new(),
            folded_through: rebuilt.folded_through,
        };
        *self = rebuilt;
        Ok(outcome)
    }

    /// Return at most `limit` items (rows plus removals), ordered by `(rev, task id)`.
    /// The composite cursor can resume inside one fold, including a full rebuild.
    /// Tombstones survive the entire epoch and are saved in the cache: if a row leaves
    /// between pages, its newer tombstone remains ahead of the client's cursor even if
    /// the client had not seen the old row. A deletion of an unseen row is a harmless no-op.
    /// A later reopen replaces that tombstone with the newer row; the latest state wins.
    pub fn delta_since(&self, epoch: u64, cursor: &BoardCursor, limit: usize) -> DeltaOutcome {
        use std::ops::Bound;
        if epoch != self.epoch || cursor.rev > self.folded_through.0 {
            return DeltaOutcome::Reset { epoch: self.epoch };
        }
        let mut delta = BoardDelta {
            epoch,
            rows: Vec::new(),
            left: Vec::new(),
            cursor: cursor.clone(),
            more: false,
        };
        let mut pending = self
            .revisions
            .range(cursor.rev..)
            .filter(|(rev, _)| **rev > cursor.rev || cursor.after.is_some())
            .flat_map(|(&rev, ids)| {
                let after = if rev == cursor.rev {
                    cursor
                        .after
                        .as_ref()
                        .map_or(Bound::Unbounded, Bound::Excluded)
                } else {
                    Bound::Unbounded
                };
                ids.range::<String, _>((after, Bound::Unbounded))
                    .map(move |id| (rev, id))
            })
            .peekable();
        for _ in 0..limit {
            let Some((rev, id)) = pending.next() else {
                break;
            };
            if let Some(row) = self.rows.get(id) {
                delta.rows.push(row.clone());
            } else {
                delta.left.push(id.clone());
            }
            delta.cursor = BoardCursor {
                rev,
                after: Some(id.clone()),
            };
        }
        delta.more = pending.peek().is_some();
        if !delta.more && limit > 0 {
            delta.cursor = BoardCursor {
                rev: self.folded_through.0,
                after: None,
            };
        }
        DeltaOutcome::Delta(delta)
    }

    fn index(&mut self, rev: u64, id: &str) {
        self.revisions.entry(rev).or_default().insert(id.into());
    }

    fn unindex_id(&mut self, id: &str) {
        let rev = self
            .rows
            .get(id)
            .map(|row| row.rev)
            .or_else(|| self.left.get(id).copied());
        if let Some(rev) = rev {
            if let Some(ids) = self.revisions.get_mut(&rev) {
                ids.remove(id);
            }
            if self.revisions.get(&rev).is_some_and(BTreeSet::is_empty) {
                self.revisions.remove(&rev);
            }
        }
    }

    fn replace_inputs(&mut self, id: &str, inputs: BTreeSet<String>) {
        if let Some(previous) = self.inputs.remove(id) {
            for input in previous {
                if let Some(dependents) = self.dependents.get_mut(&input) {
                    dependents.remove(id);
                }
                if self.dependents.get(&input).is_some_and(BTreeSet::is_empty) {
                    self.dependents.remove(&input);
                }
            }
        }
        for input in &inputs {
            self.dependents
                .entry(input.clone())
                .or_default()
                .insert(id.into());
        }
        self.inputs.insert(id.into(), inputs);
    }
}

fn closed(status: Status) -> bool {
    matches!(status, Status::Done | Status::Dropped)
}

fn new_epoch() -> Result<u64> {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow::anyhow!("choosing board epoch: {error}"))?;
    Ok(u64::from_ne_bytes(bytes))
}

fn affected_scope(scope: &ChangeScope, ids: &mut BTreeSet<String>) {
    match scope {
        ChangeScope::Task { num, .. } => {
            ids.insert(format!("T-{:03}", num.0));
        }
        ChangeScope::Events {
            task: Some(task), ..
        }
        | ChangeScope::Artifact { task }
        | ChangeScope::Lifecycle { task }
        | ChangeScope::Condition { task, .. } => {
            ids.insert(task.clone());
        }
        ChangeScope::Record { key, .. } => {
            // Task-scoped record families use a task id, optionally followed by a keyed
            // container path. Workspace records do not affect this head-only model.
            let id = key.split('/').next().unwrap_or(key);
            if id
                .strip_prefix("T-")
                .is_some_and(|num| !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()))
            {
                ids.insert(id.into());
            }
        }
        ChangeScope::Events { task: None, .. }
        | ChangeScope::Blob { .. }
        | ChangeScope::Scratch { .. }
        | ChangeScope::Aggregate { .. } => {}
    }
}
