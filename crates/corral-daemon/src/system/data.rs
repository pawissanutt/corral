//! Immutable data-model snapshots selected by an explicit commit. The default
//! (moving project head) uses the existing map observation instead.
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use corral_facts::{DataFacts, Revision};

use super::{Snapshot, Stage, System};
use crate::{ops::WorkerBusy, state::Shared, views::NotFound};

const DATA_SNAPSHOTS: usize = 32;
pub(super) const RETRY_DELAY: Duration = Duration::from_secs(2);

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DataKey {
    pub project: String,
    pub commit: String,
}

pub(crate) type DataSnapshot = Snapshot<DataFacts>;

pub(super) struct DataEntry {
    pub snapshot: DataSnapshot,
    pub pending: Option<Stage>,
    pub enrich: Option<Revision>,
    pub retry_at: Option<Instant>,
    touched: u64,
}

impl System {
    pub(crate) fn data_at(&self, shared: &Shared, project: &str, at: &str) -> Result<DataSnapshot> {
        ensure!(
            matches!(at.len(), 40 | 64) && at.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid: expected a full Git commit ID"
        );
        self.reload(shared)?;
        let mut cache = self.queue.lock();
        let config = cache
            .configuration
            .as_ref()
            .filter(|_| !cache.stopped)
            .context("system configuration unavailable")?;
        ensure!(
            config.source.projects.contains_key(project),
            NotFound("project not found")
        );
        let key = DataKey {
            project: project.to_owned(),
            commit: at.to_ascii_lowercase(),
        };
        let revision = cache.stamp();
        if let Some(entry) = cache.data.get_mut(&key) {
            entry.touched = revision;
            // The selected object may become available after a fetch.
            if entry.retry_at.is_some_and(|at| at <= Instant::now()) {
                entry.retry_at = None;
                entry.snapshot = Snapshot::pending(revision);
                entry.pending = Some(Stage::Builtin);
                self.queue.ready.notify_one();
            }
            return Ok(entry.snapshot.clone());
        }
        let retired = if cache.data.len() >= DATA_SNAPSHOTS {
            let oldest = cache
                .data
                .iter()
                .filter(|(_, entry)| !entry.snapshot.building)
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(key, _)| key.clone())
                .ok_or(WorkerBusy)?;
            cache.data.remove(&oldest)
        } else {
            None
        };
        let snapshot = Snapshot::pending(revision);
        cache.data.insert(
            key,
            DataEntry {
                snapshot: snapshot.clone(),
                pending: Some(Stage::Builtin),
                enrich: None,
                retry_at: None,
                touched: revision,
            },
        );
        self.queue.ready.notify_one();
        drop(cache);
        drop(retired);
        Ok(snapshot)
    }
}
