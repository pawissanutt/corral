//! Immutable observations for System and Review. Reads may schedule work, but
//! the worker alone reads Git and calls providers.
mod data;
mod diagram;
mod lenses;
mod worker;

use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{Arc, Condvar, Mutex, MutexGuard},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use corral_engine::{Config, Store, config::Slot, facts::FactsReport};
use corral_facts::{ProjectDelta, ProjectFacts, Revision, TreeFacts};
use corral_protocol::app::APP_CONTROL_MAX;
use corral_types::{
    Event, EventKind, Role, SourceExcerpt, SystemRow, SystemStatus, lenses::LensImpact,
};
use data::{DataEntry, DataKey};
use lenses::{LensEntry, LensKey};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    state::Shared,
    views::{NotFound, json_len},
};

pub(crate) use worker::start;

const REVIEWS: usize = 32;
const ERROR_BYTES: usize = 1024;

#[derive(Clone)]
pub(crate) struct Snapshot<Facts = ProjectFacts> {
    pub head: Option<String>,
    pub building: bool,
    pub revision: u64,
    pub facts: Option<Arc<Facts>>,
    pub provider: Option<Arc<FactsReport>>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Impact {
    pub project: String,
    pub task: String,
    pub base: String,
    pub base_estimated: bool,
    pub head: String,
    pub building: bool,
    pub revision: u64,
    pub facts: Option<Arc<ImpactFacts>>,
    pub provider: Option<Arc<FactsReport>>,
    pub error: Option<String>,
    /// Optional already-enriched view of the same head; structural comparisons
    /// use the built-in trees below, and never wait for a snapshot provider.
    pub head_snapshot: Option<Arc<ProjectFacts>>,
}

pub(crate) struct ImpactFacts {
    pub delta: ProjectDelta,
    pub base: Arc<TreeFacts>,
    pub head: Arc<TreeFacts>,
    pub lenses: Option<Arc<[LensImpact]>>,
    /// Parallel to the bounded prefix of `delta.data` considered for excerpts.
    pub data_excerpts: Arc<[Option<SourceExcerpt>]>,
}

#[derive(Default)]
pub(crate) struct System {
    queue: Arc<Queue>,
}

#[derive(Default)]
struct Queue {
    cache: Mutex<Cache>,
    ready: Condvar,
}

#[derive(Default)]
struct Cache {
    started: bool,
    stopped: bool,
    finished: bool,
    all: bool,
    revision: u64,
    configuration: Option<Arc<Configuration>>,
    // An in-flight job saw a config change. Reload even if the owner has since
    // restored the old bytes; that job's result was discarded.
    reload_required: bool,
    maps: BTreeMap<String, MapEntry>,
    reviews: BTreeMap<String, ReviewEntry>,
    lenses: BTreeMap<LensKey, LensEntry>,
    data: BTreeMap<DataKey, DataEntry>,
    // Keep the latest answer even if a review was the project's first request
    // or that particular review was later evicted from the detail cache.
    extension_answers: BTreeMap<String, ExtensionAnswers>,
}

#[derive(Default)]
struct ExtensionAnswers {
    review_provided: Option<Vec<String>>,
    last_error: Option<String>,
}

struct Configuration {
    fingerprint: [u8; 32],
    source: Config,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Builtin,
    Enrich,
}

struct MapEntry {
    snapshot: Snapshot,
    pending: Option<Stage>,
    /// Set when Git has resolved the base (including an inferred branch).
    branch: Option<String>,
    row_revision: u64,
}

struct ReviewEntry {
    submission: Submission,
    impact: Impact,
    pending: Option<Stage>,
    revisions: Option<Arc<(Revision, Revision)>>,
    touched: u64,
}

#[derive(Clone, PartialEq, Eq)]
struct Submission {
    project: String,
    task: String,
    run: String,
    base: String,
    head: String,
    legacy: bool,
}

impl System {
    pub(crate) fn stop(&self) {
        let retired = {
            let mut cache = self.queue.lock();
            cache.stopped = true;
            self.queue.ready.notify_all();
            // Don't hold the queue lock while releasing potentially large facts.
            (
                std::mem::take(&mut cache.maps),
                std::mem::take(&mut cache.reviews),
                std::mem::take(&mut cache.lenses),
                std::mem::take(&mut cache.data),
            )
        };
        drop(retired);
    }

    /// Give a cancelled provider time to finish cleanup without depending on
    /// completion of an uninterruptible Git read.
    pub(crate) fn wait_stopped(&self, timeout: Duration) {
        drop(
            self.queue
                .ready
                .wait_timeout_while(self.queue.lock(), timeout, |cache| {
                    cache.started && !cache.finished
                })
                .unwrap_or_else(|error| error.into_inner()),
        );
    }

    /// Poll only once a read has requested facts. Config changes rebuild the
    /// activated maps, not projects that have never been opened.
    pub(crate) fn refresh(&self, shared: &Shared) -> Result<()> {
        if self.queue.lock().configuration.is_some() {
            self.reload(shared)?;
        }
        Ok(())
    }

    fn reload(&self, shared: &Shared) -> Result<()> {
        // Read bytes and fingerprint them together so a replacement racing this
        // read cannot label facts built from one config with the other's key.
        for _ in 0..3 {
            let expected = shared.config_fingerprint();
            {
                let cache = self.queue.lock();
                if cache.stopped
                    || (!cache.reload_required
                        && cache
                            .configuration
                            .as_ref()
                            .is_some_and(|config| Some(config.fingerprint) == expected))
                {
                    return Ok(());
                }
            }
            let path = shared.root().join(crate::STATE_DIR).join("corral.toml");
            let text =
                fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
            let fingerprint = Sha256::digest(text.as_bytes()).into();
            if Some(fingerprint) != expected {
                shared.refresh_config()?;
                continue;
            }
            let source =
                Config::parse(&text).with_context(|| format!("parse {}", path.display()))?;
            let mut cache = self.queue.lock();
            if shared.config_fingerprint() != expected {
                continue;
            }
            if !cache.stopped
                && (cache.reload_required
                    || cache
                        .configuration
                        .as_ref()
                        .is_none_or(|config| config.fingerprint != fingerprint))
            {
                let retired = cache.configure(Configuration {
                    fingerprint,
                    source,
                });
                shared.publish();
                self.queue.ready.notify_one();
                drop(cache);
                drop(retired);
            }
            return Ok(());
        }
        bail!("configuration changed while reading system facts; retry")
    }

    pub(crate) fn observe(&self, events: &[Event]) {
        let mut retired = Vec::new();
        let mut retired_lenses = Vec::new();
        let mut cache = self.queue.lock();
        for event in events
            .iter()
            .filter(|event| event.kind == EventKind::Landed)
        {
            let Some(project) = event.project.as_deref() else {
                continue;
            };
            // Like the map, moving selectors are observed again on a Corral
            // landing or config change, without resolving Git on the read path.
            let keys: Vec<_> = cache
                .lenses
                .iter()
                .filter(|(key, entry)| {
                    key.project == project
                        && !entry
                            .snapshot
                            .commit
                            .as_deref()
                            .is_some_and(|commit| key.pinned(commit))
                })
                .map(|(key, _)| key.clone())
                .collect();
            for key in keys {
                retired_lenses.extend(cache.requeue_lenses(&key));
            }
            let Some(entry) = cache.maps.get(project) else {
                continue;
            };
            let configured = cache.configuration.as_ref().and_then(|config| {
                config
                    .source
                    .projects
                    .get(project)
                    .and_then(|settings| settings.base.as_deref())
                    .or(config.source.defaults.base.as_deref())
            });
            let branch = configured.or(entry.branch.as_deref());
            let landed = event.detail.get("base").and_then(serde_json::Value::as_str);
            if branch.zip(landed).is_some_and(|(base, onto)| base != onto) {
                continue;
            }
            let head = event.detail.get("sha").and_then(serde_json::Value::as_str);
            if head.is_some()
                && entry.snapshot.head.as_deref() == head
                && entry.snapshot.facts.is_some()
            {
                continue;
            }
            retired.extend(cache.requeue_map(project));
        }
        if !retired.is_empty() || !retired_lenses.is_empty() {
            self.queue.ready.notify_one();
        }
    }

    pub(crate) fn rows(&self, shared: &Shared) -> Result<Vec<SystemRow>> {
        self.reload(shared)?;
        let mut cache = self.queue.lock();
        if cache.stopped {
            return Ok(Vec::new());
        }
        let config = cache
            .configuration
            .as_ref()
            .context("system configuration unavailable")?
            .clone();
        if !cache.all {
            cache.all = true;
            for project in config.source.projects.keys() {
                cache.activate(project);
            }
            self.queue.ready.notify_one();
        }
        Ok(cache
            .maps
            .iter()
            .map(|(project, entry)| {
                let answers = cache.extension_answers.get(project);
                SystemRow {
                    project: project.clone(),
                    head: entry.snapshot.head.clone(),
                    status: if entry.snapshot.building {
                        SystemStatus::Building
                    } else {
                        SystemStatus::Ready
                    },
                    extension: config
                        .source
                        .extension_for_slot(project, Slot::Facts)
                        .map(|(name, _)| name.to_owned()),
                    snapshot_provided: entry.snapshot.provider.as_deref().and_then(provided),
                    review_provided: answers.and_then(|answer| answer.review_provided.clone()),
                    last_extension_error: answers.and_then(|answer| answer.last_error.clone()),
                    error: entry.snapshot.error.clone(),
                    revision: entry.row_revision,
                }
            })
            .collect())
    }

    pub(crate) fn map(&self, shared: &Shared, project: &str) -> Result<Snapshot> {
        self.reload(shared)?;
        let mut cache = self.queue.lock();
        let config = cache
            .configuration
            .as_ref()
            .filter(|_| !cache.stopped)
            .context("system configuration unavailable")?;
        if !config.source.projects.contains_key(project) {
            return Err(NotFound("project not found").into());
        }
        if cache.activate(project) {
            shared.publish();
            self.queue.ready.notify_one();
        }
        cache
            .maps
            .get(project)
            .map(|entry| entry.snapshot.clone())
            .context("system map was not queued")
    }

    pub(crate) fn review(&self, shared: &Shared, task: &str) -> Result<Impact> {
        self.reload(shared)?;
        let submission = submitted(shared.root(), task)?;
        let mut cache = self.queue.lock();
        let config = cache
            .configuration
            .as_ref()
            .filter(|_| !cache.stopped)
            .context("system configuration unavailable")?;
        if !config.source.projects.contains_key(&submission.project) {
            return Err(NotFound("project not found").into());
        }
        let touched = cache.stamp();
        if let Some(entry) = cache.reviews.get_mut(task)
            && entry.submission == submission
        {
            entry.touched = touched;
            return Ok(entry.impact.clone());
        }
        let evicted = if cache.reviews.len() >= REVIEWS && !cache.reviews.contains_key(task) {
            let oldest = cache
                .reviews
                .iter()
                .filter(|(_, entry)| !entry.impact.building && entry.revisions.is_none())
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(id, _)| id.clone())
                .ok_or(crate::ops::WorkerBusy)?;
            cache.reviews.remove(&oldest)
        } else {
            None
        };
        let impact = Impact {
            project: submission.project.clone(),
            task: submission.task.clone(),
            base: submission.base.clone(),
            base_estimated: submission.legacy,
            head: submission.head.clone(),
            building: true,
            revision: touched,
            facts: None,
            provider: None,
            error: None,
            head_snapshot: None,
        };
        let replaced = cache.reviews.insert(
            task.to_owned(),
            ReviewEntry {
                submission,
                impact: impact.clone(),
                pending: Some(Stage::Builtin),
                revisions: None,
                touched,
            },
        );
        self.queue.ready.notify_one();
        drop(cache);
        drop((evicted, replaced));
        Ok(impact)
    }
}

impl Drop for System {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Queue {
    fn lock(&self) -> MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(|error| error.into_inner())
    }
}

impl Cache {
    fn stamp(&mut self) -> u64 {
        self.revision += 1;
        self.revision
    }

    fn configure(&mut self, config: Configuration) -> impl Sized + use<> {
        let maps = std::mem::take(&mut self.maps);
        let reviews = std::mem::take(&mut self.reviews);
        let lenses = std::mem::take(&mut self.lenses);
        let data = std::mem::take(&mut self.data);
        for project in config.source.projects.keys() {
            if self.all || maps.contains_key(project) {
                self.activate(project);
            }
        }
        self.configuration = Some(Arc::new(config));
        self.extension_answers.clear();
        self.reload_required = false;
        (maps, reviews, lenses, data)
    }

    /// Return whether a request was newly queued.
    fn activate(&mut self, project: &str) -> bool {
        if self.maps.contains_key(project) {
            return false;
        }
        let revision = self.stamp();
        self.maps.insert(
            project.to_owned(),
            MapEntry {
                snapshot: Snapshot::pending(revision),
                pending: Some(Stage::Builtin),
                branch: None,
                row_revision: revision,
            },
        );
        true
    }

    fn requeue_map(&mut self, project: &str) -> Option<Snapshot> {
        let revision = self.stamp();
        let entry = self.maps.get_mut(project)?;
        entry.pending = Some(Stage::Builtin);
        entry.row_revision = revision;
        Some(std::mem::replace(
            &mut entry.snapshot,
            Snapshot::pending(revision),
        ))
    }

    pub(super) fn detail_row(&mut self, project: &str, revision: u64) {
        if let Some(entry) = self.maps.get_mut(project) {
            entry.row_revision = revision;
        }
    }

    fn review_row(&mut self, project: &str, revision: u64, provider: Option<&FactsReport>) {
        self.detail_row(project, revision);
        if let Some(provider) = provider {
            let answers = self
                .extension_answers
                .entry(project.to_owned())
                .or_default();
            answers.review_provided = provided(provider);
            answers.last_error.clone_from(&provider.error);
        }
    }
}

impl<Facts> Snapshot<Facts> {
    fn pending(revision: u64) -> Self {
        Self {
            head: None,
            building: true,
            revision,
            facts: None,
            provider: None,
            error: None,
        }
    }
}

fn submitted(root: &Path, task: &str) -> Result<Submission> {
    let store = Store::open_read_only(&root.join(crate::STATE_DIR).join("corral.db"))?;
    store.read_snapshot(|| {
        let task = store.get_task(task)?.context(NotFound("task not found"))?;
        let run = store
            .latest_run(&task.id, Role::Worker)?
            .filter(|run| run.summary.is_some())
            .context(NotFound("task has no submitted worker run"))?;
        let submitted = store
            .submitted_revision(&task.id, &run.id)?
            .context(NotFound("task has no submitted worker run"))?;
        let project = submitted
            .project
            .or(task.project)
            .context(NotFound("project not found"))?;
        let legacy = submitted.base_sha.is_none();
        Ok(Submission {
            project,
            task: task.id,
            run: run.id,
            base: submitted.base_sha.unwrap_or(run.base_sha),
            head: submitted.sha,
            legacy,
        })
    })
}

fn provided(report: &FactsReport) -> Option<Vec<String>> {
    (report.error.is_none() || !report.coverage.provided.is_empty())
        .then(|| report.coverage.provided.clone())
}

fn bounded_error(mut text: String) -> String {
    if text.len() > ERROR_BYTES {
        text.truncate(text.floor_char_boundary(ERROR_BYTES - '…'.len_utf8()));
        text.push('…');
    }
    text
}

/// Keep the built-in observation if a provider reply cannot be paged. Reserve
/// room for the detail envelope, built-in metadata and row discriminators.
fn bounded_report(mut report: FactsReport, rows_fit: impl FnOnce() -> Result<()>) -> FactsReport {
    if report.error.is_none() {
        let fit = (|| -> Result<()> {
            ensure!(
                json_len(&report)? <= APP_CONTROL_MAX / 4,
                "facts provider metadata exceeds the detail budget"
            );
            rows_fit()
        })();
        if let Err(error) = fit {
            let coverage = &mut report.coverage;
            coverage.not_provided.append(&mut coverage.provided);
            coverage.skipped.clear();
            coverage.omitted.clear();
            coverage.dropped_unknown_files = 0;
            report.error = Some(error.to_string());
        }
    }
    report.error = report.error.map(bounded_error);
    report
}

fn provider_row_fits(row: &impl Serialize) -> Result<()> {
    ensure!(
        json_len(row)? <= APP_CONTROL_MAX / 2,
        "facts provider row exceeds the detail budget"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landed(project: &str, base: &str) -> Event {
        Event {
            seq: 1,
            ts: jiff::Timestamp::now(),
            task: None,
            project: Some(project.into()),
            run: None,
            kind: EventKind::Landed,
            actor: corral_types::Actor::Auto,
            detail: serde_json::json!({"base":base,"sha":"new"}),
        }
    }

    #[test]
    fn only_a_landing_on_an_activated_projects_base_requeues_its_map() {
        let system = System::default();
        let old = {
            let mut cache = system.queue.lock();
            let mut source = Config::default();
            source.defaults.base = Some("main".into());
            source.projects.insert("active".into(), Default::default());
            source
                .projects
                .insert("inactive".into(), Default::default());
            cache.configure(Configuration {
                fingerprint: [0; 32],
                source,
            });
            cache.activate("active");
            cache.maps["active"].snapshot.revision
        };
        system.observe(&[landed("inactive", "main"), landed("active", "other")]);
        {
            let cache = system.queue.lock();
            assert_eq!(cache.maps.len(), 1);
            assert_eq!(cache.maps["active"].snapshot.revision, old);
        }
        system.observe(&[landed("active", "main")]);
        assert!(system.queue.lock().maps["active"].snapshot.revision > old);
    }
}
