//! One worker per daemon. Pending built-ins go first; a running provider is
//! never waited on by a read, engine operation, or daemon shutdown.
use std::{
    path::PathBuf,
    sync::{Arc, Weak},
    thread,
};

use anyhow::{Context, Result, ensure};
use corral_engine::{config::Slot, facts::FactsReader};
use corral_facts::Revision;

use super::{
    Cache, Configuration, Impact, ImpactFacts, Queue, Snapshot, Stage, Submission, bounded_error,
    bounded_report, provider_row_fits, submitted,
};
use crate::{state::Shared, views::NotFound};

pub(crate) fn start(shared: &Arc<Shared>) -> Result<()> {
    let queue = &shared.system.queue;
    let mut cache = queue.lock();
    if cache.started || cache.stopped {
        return Ok(());
    }
    let worker = Worker {
        root: shared.root().to_path_buf(),
        shared: Arc::downgrade(shared),
        queue: Arc::clone(queue),
    };
    // Intentionally detached: cancellation is cooperative, and an unbounded
    // Git read cannot make the daemon wait on a join.
    thread::Builder::new()
        .name("corrald-facts".into())
        .spawn(move || {
            let _completion = Completion(&worker.queue);
            worker.run();
        })
        .context("start daemon facts worker")?;
    cache.started = true;
    Ok(())
}

struct Completion<'a>(&'a Queue);

impl Drop for Completion<'_> {
    fn drop(&mut self) {
        self.0.lock().finished = true;
        self.0.ready.notify_all();
    }
}

struct Worker {
    root: PathBuf,
    shared: Weak<Shared>,
    queue: Arc<Queue>,
}

struct Job {
    config: Arc<Configuration>,
    work: Work,
}

enum Work {
    Map {
        project: String,
        stage: Stage,
        snapshot: Snapshot,
        branch: Option<String>,
    },
    Review {
        submission: Submission,
        stage: Stage,
        impact: Impact,
        revisions: Option<Arc<(Revision, Revision)>>,
    },
}

impl Worker {
    fn run(&self) {
        while let Some(job) = self.queue.next() {
            if !self.current(&job) {
                continue;
            }
            match &job.work {
                Work::Map {
                    project,
                    stage: Stage::Builtin,
                    ..
                } => self.map_builtin(&job, project),
                Work::Map {
                    project,
                    stage: Stage::Enrich,
                    snapshot,
                    branch,
                } => {
                    self.map_enrich(&job, project, snapshot, branch.as_deref());
                }
                Work::Review {
                    submission,
                    stage: Stage::Builtin,
                    ..
                } => {
                    self.review_builtin(&job, submission);
                }
                Work::Review {
                    submission,
                    stage: Stage::Enrich,
                    impact,
                    revisions,
                } => {
                    self.review_enrich(&job, submission, impact, revisions.as_deref());
                }
            }
        }
    }

    fn current(&self, job: &Job) -> bool {
        self.shared
            .upgrade()
            .is_some_and(|shared| job.current(&mut self.queue.lock(), &shared))
    }

    fn cancelled(&self, job: &Job) -> bool {
        let cache = self.queue.lock();
        cache.stopped || cache.reload_required || !job.matches(&cache)
    }

    fn publish<T>(&self, job: &Job, apply: impl FnOnce(&mut Cache) -> T) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let mut cache = self.queue.lock();
        if !job.current(&mut cache, &shared) {
            return;
        }
        let retired = apply(&mut cache);
        shared.publish();
        drop(cache);
        drop(retired);
    }

    fn reader<'a>(&self, job: &'a Job, project: &'a str) -> Result<FactsReader<'a>> {
        FactsReader::new(&self.root, &job.config.source, project)
    }

    fn map_builtin(&self, job: &Job, project: &str) {
        let mut branch = None;
        let mut head = None;
        let facts = (|| {
            let reader = self.reader(job, project)?;
            branch = reader.base_branch()?;
            let tip = head.insert(reader.project_tip(branch.as_deref())?);
            reader.builtin_snapshot(&reader.resolve_revision(tip)?)
        })();
        if let Some(head) = head.as_deref()
            && self.requeue_if_tip_moved(job, project, branch.as_deref(), head)
        {
            return;
        }
        let (facts, error) = match facts {
            Ok(facts) => (Some(Arc::new(facts)), None),
            Err(error) => (None, Some(bounded_error(format!("{error:#}")))),
        };
        let enrich = facts.is_some() && job.has_provider(project);
        self.publish(job, move |cache| {
            let revision = cache.stamp();
            let Some(entry) = cache.maps.get_mut(project) else {
                return;
            };
            entry.branch = branch;
            entry.snapshot = Snapshot {
                head,
                building: false,
                revision,
                facts,
                provider: None,
                error,
            };
            entry.row_revision = revision;
            entry.pending = enrich.then_some(Stage::Enrich);
        });
    }

    fn map_enrich(&self, job: &Job, project: &str, snapshot: &Snapshot, branch: Option<&str>) {
        let (Some(facts), Some(head)) = (&snapshot.facts, &snapshot.head) else {
            return;
        };
        if self.requeue_if_tip_moved(job, project, branch, head) {
            return;
        }
        let cancelled = || self.cancelled(job);
        let Ok(reader) = self.reader(job, project) else {
            return;
        };
        let reader = reader.with_cancellation(&cancelled);
        let mut enriched = (**facts).clone();
        if !self.current(job) {
            return;
        }
        let Some(report) = reader.enrich_snapshot(&mut enriched) else {
            return;
        };
        let report = bounded_report(report, || {
            enriched
                .structure
                .files
                .iter()
                .flat_map(|file| &file.definitions)
                .try_for_each(provider_row_fits)
        });
        if self.requeue_if_tip_moved(job, project, branch, head) {
            return;
        }
        let enriched = if report.error.is_none() {
            Arc::new(enriched)
        } else {
            Arc::clone(facts)
        };
        self.publish(job, move |cache| {
            let revision = cache.stamp();
            let mut retired = Vec::new();
            let Some(entry) = cache.maps.get_mut(project) else {
                return retired;
            };
            entry.snapshot.revision = revision;
            entry.snapshot.facts = Some(Arc::clone(&enriched));
            entry.snapshot.provider = Some(Arc::clone(&report));
            entry.last_extension_error.clone_from(&report.error);
            entry.row_revision = revision;
            if report.error.is_none() {
                for review in cache.reviews.values_mut().filter(|review| {
                    review.submission.project == project
                        && review.submission.head == *head
                        && review.impact.facts.is_some()
                }) {
                    retired.extend(review.impact.head_snapshot.replace(Arc::clone(&enriched)));
                    review.impact.revision = revision;
                }
            }
            retired
        });
    }

    fn requeue_if_tip_moved(
        &self,
        job: &Job,
        project: &str,
        branch: Option<&str>,
        head: &str,
    ) -> bool {
        if !self.current(job) {
            return true;
        }
        let unchanged = self.reader(job, project).and_then(|reader| {
            let current = reader.base_branch()?;
            Ok(current.as_deref() == branch && reader.project_tip(current.as_deref())? == head)
        });
        if matches!(unchanged, Ok(true)) {
            return false;
        }
        // If the ref vanished, the next built-in reports the Git error instead
        // of letting the old successful payload masquerade as the current map.
        self.publish(job, |cache| cache.requeue_map(project));
        true
    }

    fn review_builtin(&self, job: &Job, submission: &Submission) {
        if !self.check_submission(job, submission) {
            return;
        }
        let result = (|| {
            let reader = self.reader(job, &submission.project)?;
            let base_sha = if submission.legacy {
                reader.legacy_submitted_base(&submission.base, &submission.head)?
            } else {
                submission.base.clone()
            };
            let base = reader.resolve_revision(&base_sha)?;
            let head = reader.resolve_revision(&submission.head)?;
            ensure!(
                base.commit() == base_sha && head.commit() == submission.head,
                "submitted run lacks full commit IDs"
            );
            let before = Arc::new(reader.builtin_tree(&base)?);
            let after = if base.tree() == head.tree() {
                Arc::clone(&before)
            } else {
                Arc::new(reader.builtin_tree(&head)?)
            };
            let delta = reader.builtin_delta(&base, &head)?;
            Ok::<_, anyhow::Error>((
                base,
                head,
                ImpactFacts {
                    delta,
                    base: before,
                    head: after,
                },
            ))
        })();
        if !self.check_submission(job, submission) {
            return;
        }
        let (facts, revisions, error) = match result {
            Ok((base, head, facts)) => (Some(Arc::new(facts)), Some((base, head)), None),
            Err(error) => (None, None, Some(bounded_error(format!("{error:#}")))),
        };
        let enrich = facts.is_some() && job.has_provider(&submission.project);
        self.publish(job, move |cache| {
            let revision = cache.stamp();
            let head_snapshot = facts.as_ref().and_then(|_| cache.enriched_head(submission));
            let Some(entry) = cache.reviews.get_mut(&submission.task) else {
                return;
            };
            entry.impact.revision = revision;
            entry.impact.building = false;
            if let Some((base, _)) = &revisions {
                entry.impact.base = base.commit().to_owned();
            }
            entry.impact.facts = facts;
            entry.impact.error = error;
            entry.impact.head_snapshot = head_snapshot;
            entry.revisions = revisions.filter(|_| enrich).map(Arc::new);
            entry.pending = enrich.then_some(Stage::Enrich);
            cache.review_row(&submission.project, revision, None);
        });
    }

    fn review_enrich(
        &self,
        job: &Job,
        submission: &Submission,
        impact: &Impact,
        revisions: Option<&(Revision, Revision)>,
    ) {
        let (Some(facts), Some((base, head))) = (&impact.facts, revisions) else {
            return;
        };
        if !self.check_submission(job, submission) {
            return;
        }
        let cancelled = || self.cancelled(job);
        let Ok(reader) = self.reader(job, &submission.project) else {
            return;
        };
        let reader = reader.with_cancellation(&cancelled);
        let mut delta = facts.delta.clone();
        if !self.current(job) {
            return;
        }
        let Some(report) = reader.enrich_delta(base, head, &mut delta) else {
            return;
        };
        let report = bounded_report(report, || {
            delta
                .changes
                .iter()
                .flatten()
                .try_for_each(provider_row_fits)?;
            delta
                .dependents
                .iter()
                .flatten()
                .try_for_each(provider_row_fits)?;
            delta.tests.iter().flatten().try_for_each(provider_row_fits)
        });
        if !self.check_submission(job, submission) {
            return;
        }
        let enriched = if report.error.is_none() {
            Arc::new(ImpactFacts {
                delta,
                base: Arc::clone(&facts.base),
                head: Arc::clone(&facts.head),
            })
        } else {
            Arc::clone(facts)
        };
        self.publish(job, move |cache| {
            let revision = cache.stamp();
            let Some(entry) = cache.reviews.get_mut(&submission.task) else {
                return;
            };
            entry.impact.revision = revision;
            entry.impact.facts = Some(enriched);
            entry.impact.provider = Some(Arc::clone(&report));
            entry.revisions = None;
            cache.review_row(&submission.project, revision, Some(&report));
        });
    }

    fn check_submission(&self, job: &Job, submission: &Submission) -> bool {
        if !self.current(job) {
            return false;
        }
        match submitted(&self.root, &submission.task) {
            Ok(current) if current == *submission => true,
            Ok(_) => {
                self.retire_review(job, submission);
                false
            }
            Err(error) => {
                if !error.is::<NotFound>() {
                    eprintln!("corrald check submitted facts revision: {error:#}");
                }
                self.retire_review(job, submission);
                false
            }
        }
    }

    fn retire_review(&self, job: &Job, submission: &Submission) {
        self.publish(job, |cache| {
            let retired = cache.reviews.remove(&submission.task);
            let revision = cache.stamp();
            cache.review_row(&submission.project, revision, None);
            retired
        });
    }
}

impl Queue {
    fn next(&self) -> Option<Job> {
        let mut cache = self.lock();
        loop {
            if cache.stopped {
                return None;
            }
            if !cache.reload_required
                && let Some(job) = cache.next()
            {
                return Some(job);
            }
            cache = self
                .ready
                .wait(cache)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

impl Cache {
    fn next(&mut self) -> Option<Job> {
        enum Kind {
            Map,
            Review,
        }
        let maps = self.maps.iter().filter_map(|(id, entry)| {
            Some((entry.pending?, entry.snapshot.revision, id, Kind::Map))
        });
        let reviews = self.reviews.iter().filter_map(|(id, entry)| {
            Some((entry.pending?, entry.impact.revision, id, Kind::Review))
        });
        let (stage, _, id, kind) = maps
            .chain(reviews)
            .min_by_key(|(stage, revision, _, _)| (*stage, *revision))?;
        let id = id.clone();
        let config = Arc::clone(self.configuration.as_ref()?);
        let work = match kind {
            Kind::Map => {
                let entry = self.maps.get_mut(&id)?;
                entry.pending = None;
                Work::Map {
                    project: id,
                    stage,
                    snapshot: entry.snapshot.clone(),
                    branch: entry.branch.clone(),
                }
            }
            Kind::Review => {
                let entry = self.reviews.get_mut(&id)?;
                entry.pending = None;
                Work::Review {
                    submission: entry.submission.clone(),
                    stage,
                    impact: entry.impact.clone(),
                    revisions: entry.revisions.clone(),
                }
            }
        };
        Some(Job { config, work })
    }

    fn enriched_head(&self, submission: &Submission) -> Option<Arc<corral_facts::ProjectFacts>> {
        let map = &self.maps.get(&submission.project)?.snapshot;
        if map.head.as_deref() == Some(submission.head.as_str())
            && map
                .provider
                .as_ref()
                .is_some_and(|report| report.error.is_none())
        {
            map.facts.clone()
        } else {
            None
        }
    }
}

impl Job {
    fn current(&self, cache: &mut Cache, shared: &Shared) -> bool {
        if cache.stopped || cache.reload_required || !self.matches(cache) {
            return false;
        }
        let same = shared.config_fingerprint() == Some(self.config.fingerprint);
        if !same {
            cache.reload_required = true;
        }
        same
    }

    fn matches(&self, cache: &Cache) -> bool {
        cache
            .configuration
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(&self.config, current))
            && match &self.work {
                Work::Map {
                    project, snapshot, ..
                } => cache
                    .maps
                    .get(project)
                    .is_some_and(|entry| entry.snapshot.revision == snapshot.revision),
                Work::Review {
                    submission, impact, ..
                } => cache
                    .reviews
                    .get(&submission.task)
                    .is_some_and(|entry| entry.impact.revision == impact.revision),
            }
    }

    fn has_provider(&self, project: &str) -> bool {
        self.config
            .source
            .extension_for_slot(project, Slot::Facts)
            .is_some()
    }
}
