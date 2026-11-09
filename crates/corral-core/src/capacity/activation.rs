//! A validated config write upgrades the store fence before exposing new scheduler semantics.
//! Readers require a completion receipt: a crash after fencing cannot expose partial backfill.
use anyhow::{Result, bail, ensure};
use serde::Deserialize;

use crate::{Config, GateRecord, Store, backend::HeadQuery};

const AREA: &str = "capacity-model";
const ACTIVATION: &str = "activation";

fn activated(store: &Store) -> Result<bool> {
    match store.record::<u32>(AREA, ACTIVATION)? {
        None => Ok(false),
        Some(1) => Ok(true),
        Some(version) => bail!("unsupported capacity activation version {version}"),
    }
}

#[derive(Deserialize)]
struct CheckpointJournal {
    version: u32,
    attempts: Vec<Checkpoint>,
}

#[derive(Deserialize)]
struct Checkpoint {
    id: String,
    gate: GateRecord,
}

pub(crate) fn require_config(store: &Store, config: &Config) -> Result<()> {
    if config.limits.capacity.is_some() {
        store.require_capacity_schema()?;
    }
    Ok(())
}

/// Called only under the validated config writer's dispatch/config locks, before config publication.
pub(crate) fn activate(store: &Store, first_enable: bool) -> Result<()> {
    let ready = activated(store)?;
    let backfill = first_enable || crate::schema::store_version(store)? != 5 || !ready;
    if backfill {
        store.delete_record(AREA, ACTIVATION)?;
    }
    crate::store::db::publish_capacity_fence(store.dir())?;
    if !backfill {
        return Ok(());
    }
    // Only activation reads historical aggregate containers. Hot admission uses their
    // maintained counters; journal Running facts stay reserved until reconciled.
    for head in store.heads(&HeadQuery::Open { project: None })? {
        let committed = super::head_committed(&head);
        let lease = if head.tracker {
            checkpoint_lease(store, &head)?
        } else {
            None
        };
        let (gates, reviews) = if head.summary.aggregate_attempts > 0 {
            let summary = crate::HistorySummary::of(&store.get(&head.id)?)?;
            (
                summary.held_aggregate_gates,
                summary.pending_aggregate_reviews,
            )
        } else {
            (0, 0)
        };
        if head.capacity_started != Some(committed)
            || head.capacity_checkpoint != lease
            || head.summary.held_aggregate_gates != gates
            || head.summary.pending_aggregate_reviews != reviews
        {
            // The config writer already holds dispatch; reacquiring it via update_head
            // would deadlock. Keep the ordinary task lease and guarded head write.
            let _task = store.lock(&head.id)?;
            let mut task = crate::TaskHeadMut::new(store.head(&head.id)?);
            task.capacity_checkpoint = lease;
            task.capacity_started = Some(committed);
            task.head.summary.held_aggregate_gates = gates;
            task.head.summary.pending_aggregate_reviews = reviews;
            store.write_head(task)?;
        }
    }
    Ok(())
}

/// Publish readiness only after the validated configuration has reached disk.
pub(crate) fn complete(store: &Store) -> Result<()> {
    store.save_record(AREA, ACTIVATION, &1_u32)
}

fn checkpoint_lease(store: &Store, head: &crate::TaskHead) -> Result<Option<String>> {
    let journal: Option<CheckpointJournal> = store.record("plan-checkpoints", &head.id)?;
    let Some(journal) = journal else {
        ensure!(
            head.capacity_checkpoint.is_none(),
            "checkpoint reservation for {} has no journal; reconcile it before activation",
            head.id
        );
        return Ok(None);
    };
    ensure!(
        journal.version == 1,
        "unsupported checkpoint journal version {} for {}",
        journal.version,
        head.id
    );
    let running: Vec<_> = journal
        .attempts
        .iter()
        .filter(|a| a.gate.holds_slot())
        .collect();
    ensure!(
        running.len() <= 1,
        "multiple running checkpoints for {}; reconcile before activation",
        head.id
    );
    Ok(running.first().map(|attempt| attempt.id.clone()))
}

impl Store {
    /// New scheduling is never enabled by a file edit that old writers could silently ignore.
    pub fn require_capacity_schema(&self) -> Result<()> {
        ensure!(
            crate::schema::store_version(self)? == 5,
            "independent capacity requires store fence 5; activate through a validated config write with a compatible corral"
        );
        ensure!(
            activated(self)?,
            "independent capacity activation is incomplete; resume through a validated capacity config write"
        );
        Ok(())
    }
}
