//! Lens checks at immutable Git revisions. Cache by project and revision selector,
//! not by lens ID, so list and detail reads share one check and provider answer.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use corral_engine::facts::FactsReader;
use corral_facts::Revision;
use corral_types::lenses::{LensExcerpt, LensResult};

use super::{Cache, Stage, System};
use crate::{ops::WorkerBusy, state::Shared, views::NotFound};

const LENS_SNAPSHOTS: usize = 32;
const REF_CHECK_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct LensKey {
    pub project: String,
    pub at: Option<String>,
}

impl LensKey {
    pub(super) fn pinned(&self, commit: &str) -> bool {
        self.at
            .as_deref()
            .is_some_and(|at| at.eq_ignore_ascii_case(commit))
    }
}

#[derive(Clone)]
pub(crate) struct LensSnapshot {
    pub commit: Option<String>,
    pub building: bool,
    pub revision: u64,
    pub results: Option<Arc<[LensResult]>>,
    /// Committed source around each result's located elements, in the results' order.
    pub excerpts: Option<Arc<[Vec<LensExcerpt>]>>,
    pub error: Option<String>,
}

pub(super) struct LensEntry {
    pub snapshot: LensSnapshot,
    pub pending: Option<Stage>,
    pub enrich: Option<Revision>,
    refresh_after: Instant,
    touched: u64,
}

impl System {
    pub(crate) fn lenses(
        &self,
        shared: &Shared,
        project: &str,
        at: Option<&str>,
    ) -> Result<LensSnapshot> {
        ensure!(
            at.is_none_or(|at| !at.is_empty() && at.len() <= 1024 && !at.contains('\0')),
            "invalid: invalid lens revision selector"
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
        let key = LensKey {
            project: project.to_owned(),
            at: at.map(str::to_owned),
        };
        let revision = cache.stamp();
        if let Some(entry) = cache.lenses.get_mut(&key) {
            entry.touched = revision;
            // Resolve moving selectors on the worker, including previous failures.
            // Polling an unchanged branch does not rebuild the lens or change its etag.
            if !entry.snapshot.building
                && entry.pending.is_none()
                && !entry
                    .snapshot
                    .results
                    .as_ref()
                    .zip(entry.snapshot.commit.as_deref())
                    .is_some_and(|(_, commit)| key.pinned(commit))
                && Instant::now() >= entry.refresh_after
            {
                entry.defer_refresh();
                entry.pending = Some(Stage::Builtin);
                self.queue.ready.notify_one();
            }
            return Ok(entry.snapshot.clone());
        }
        let retired = if cache.lenses.len() >= LENS_SNAPSHOTS {
            let oldest = cache
                .lenses
                .iter()
                .filter(|(_, entry)| !entry.snapshot.building)
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(key, _)| key.clone())
                .ok_or(WorkerBusy)?;
            cache.lenses.remove(&oldest)
        } else {
            None
        };
        let entry = LensEntry::pending(revision);
        let snapshot = entry.snapshot.clone();
        cache.lenses.insert(key, entry);
        self.queue.ready.notify_one();
        drop(cache);
        drop(retired);
        Ok(snapshot)
    }
}

impl LensEntry {
    fn pending(revision: u64) -> Self {
        Self {
            snapshot: LensSnapshot {
                commit: None,
                building: true,
                revision,
                results: None,
                excerpts: None,
                error: None,
            },
            pending: Some(Stage::Builtin),
            enrich: None,
            refresh_after: Instant::now() + REF_CHECK_INTERVAL,
            touched: revision,
        }
    }

    pub(super) fn defer_refresh(&mut self) {
        self.refresh_after = Instant::now() + REF_CHECK_INTERVAL;
    }
}

impl Cache {
    pub(super) fn requeue_lenses(&mut self, key: &LensKey) -> Option<LensEntry> {
        let revision = self.stamp();
        let entry = self.lenses.get_mut(key)?;
        let mut pending = LensEntry::pending(revision);
        pending.touched = entry.touched;
        let retired = std::mem::replace(entry, pending);
        self.lens_row(&key.project, revision);
        Some(retired)
    }

    pub(super) fn lens_row(&mut self, project: &str, revision: u64) {
        if let Some(entry) = self.maps.get_mut(project) {
            entry.row_revision = revision;
        }
    }
}

pub(super) fn revision(reader: &FactsReader<'_>, key: &LensKey) -> Result<Revision> {
    match &key.at {
        Some(at) => reader.resolve_revision(at),
        None => {
            let branch = reader.base_branch()?;
            reader.resolve_revision(&reader.project_tip(branch.as_deref())?)
        }
    }
}
