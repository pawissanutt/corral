//! Immutable data-model snapshots selected by an explicit commit. The default
//! (moving project head) uses the existing map observation instead.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use corral_facts::{DataFacts, Revision};

use super::{Snapshot, Stage, System, diagram::DiagramFacts};
use crate::{ops::WorkerBusy, state::Shared, views::NotFound};

const DATA_SNAPSHOTS: usize = 32;
// Diagram observations retain a whole structural snapshot, not just data shapes.
const DIAGRAM_SNAPSHOTS: usize = 4;
pub(super) const RETRY_DELAY: Duration = Duration::from_secs(2);

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DataKey {
    pub project: String,
    pub commit: String,
    pub projection: HistoricalProjection,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum HistoricalProjection {
    Data,
    Diagram,
}

#[derive(Clone)]
pub(super) enum HistoricalFacts {
    Data(Arc<DataFacts>),
    Diagram(Arc<DiagramFacts>),
}

pub(super) struct DataEntry {
    pub snapshot: Snapshot<HistoricalFacts>,
    pub pending: Option<Stage>,
    pub enrich: Option<Revision>,
    pub retry_at: Option<Instant>,
    touched: u64,
}

impl System {
    pub(crate) fn data_at(
        &self,
        shared: &Shared,
        project: &str,
        at: &str,
    ) -> Result<Snapshot<DataFacts>> {
        Ok(project_facts(
            self.historical_at(shared, project, at, HistoricalProjection::Data)?,
            |facts| match facts {
                HistoricalFacts::Data(data) => Some(Arc::clone(data)),
                HistoricalFacts::Diagram(_) => None,
            },
        ))
    }

    pub(crate) fn diagram_at(
        &self,
        shared: &Shared,
        project: &str,
        at: &str,
    ) -> Result<Snapshot<DiagramFacts>> {
        Ok(project_facts(
            self.historical_at(shared, project, at, HistoricalProjection::Diagram)?,
            |facts| match facts {
                HistoricalFacts::Diagram(tree) => Some(Arc::clone(tree)),
                HistoricalFacts::Data(_) => None,
            },
        ))
    }

    fn historical_at(
        &self,
        shared: &Shared,
        project: &str,
        at: &str,
        projection: HistoricalProjection,
    ) -> Result<Snapshot<HistoricalFacts>> {
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
            projection,
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
        let capacity = match projection {
            HistoricalProjection::Data => DATA_SNAPSHOTS,
            HistoricalProjection::Diagram => DIAGRAM_SNAPSHOTS,
        };
        let same_projection = |key: &DataKey| key.projection == projection;
        let retired = if cache.data.keys().filter(|key| same_projection(key)).count() >= capacity {
            let oldest = cache
                .data
                .iter()
                .filter(|(key, entry)| same_projection(key) && !entry.snapshot.building)
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

fn project_facts<T>(
    snapshot: Snapshot<HistoricalFacts>,
    select: impl FnOnce(&HistoricalFacts) -> Option<Arc<T>>,
) -> Snapshot<T> {
    Snapshot {
        head: snapshot.head,
        building: snapshot.building,
        revision: snapshot.revision,
        facts: snapshot.facts.as_deref().and_then(select),
        provider: snapshot.provider,
        error: snapshot.error,
    }
}
