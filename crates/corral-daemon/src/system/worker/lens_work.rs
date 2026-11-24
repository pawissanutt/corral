use std::sync::Arc;

use anyhow::Result;
use corral_engine::facts::FactsReader;
use corral_facts::{
    Revision, TreeFacts,
    lenses::{LensCoverage, check},
};
use corral_types::lenses::LensImpact;

use super::{Job, Worker};
use crate::system::{
    Stage, bounded_error,
    lenses::{self, LensKey, LensSnapshot},
};

impl Worker {
    pub(super) fn lens_builtin(&self, job: &Job, key: &LensKey, prior: &LensSnapshot) {
        let mut commit = None;
        let result = (|| -> Result<_> {
            let reader = self.reader(job, &key.project)?;
            let rev = lenses::revision(&reader, key)?;
            if prior.results.is_some() && prior.commit.as_deref() == Some(rev.commit()) {
                return Ok(None);
            }
            commit = Some(rev.commit().to_owned());
            let sources = reader.lens_sources(&rev)?;
            if sources.is_empty() {
                return Ok(Some((Vec::new(), None)));
            }
            let tree = reader.builtin_tree(&rev)?;
            let results = check(&sources, &tree, None);
            let enrich = (job.has_provider(&key.project)
                && results.iter().any(|result| result.lens.is_some()))
            .then_some(rev);
            Ok(Some((results, enrich)))
        })();
        if let Some(commit) = commit.as_deref()
            && self.requeue_if_lens_moved(job, key, commit)
        {
            return;
        }
        let (results, enrich, error) = match result {
            Ok(Some((results, enrich))) => (Some(results.into()), enrich, None),
            Ok(None) => return,
            Err(error) => (None, None, Some(bounded_error(format!("{error:#}")))),
        };
        if error.is_some() && prior.error == error && prior.commit == commit {
            return;
        }
        self.publish(job, move |cache| {
            let revision = cache.stamp();
            let Some(entry) = cache.lenses.get_mut(key) else {
                return;
            };
            entry.snapshot = LensSnapshot {
                commit,
                building: false,
                revision,
                results,
                error,
            };
            entry.pending = enrich.as_ref().map(|_| Stage::Enrich);
            entry.enrich = enrich;
            entry.defer_refresh();
            cache.lens_row(&key.project, revision);
        });
    }

    pub(super) fn lens_enrich(&self, job: &Job, key: &LensKey, rev: &Revision) {
        let commit = rev.commit();
        if self.requeue_if_lens_moved(job, key, commit) {
            return;
        }
        let cancelled = || self.cancelled(job);
        let Ok(reader) = self.reader(job, &key.project) else {
            return;
        };
        let reader = reader.with_cancellation(&cancelled);
        // Queue only the immutable revision, not a copy of the repository's
        // full structure and retained anchor blobs for every pending selector.
        let (Ok(sources), Ok(mut tree)) = (reader.lens_sources(rev), reader.builtin_tree(rev))
        else {
            return;
        };
        let (Some(report), unread_files) = reader.enrich_lens_tree(rev, &mut tree) else {
            return;
        };
        if self.requeue_if_lens_moved(job, key, commit) || report.error.is_some() {
            return;
        }
        let coverage = LensCoverage {
            provider: &report.coverage,
            unread_files: &unread_files,
        };
        let results = check(&sources, &tree, Some(coverage));
        self.publish(job, move |cache| {
            let revision = cache.stamp();
            let entry = cache.lenses.get_mut(key)?;
            entry.snapshot.revision = revision;
            let old = entry.snapshot.results.replace(results.into());
            cache.lens_row(&key.project, revision);
            old
        });
    }

    fn requeue_if_lens_moved(&self, job: &Job, key: &LensKey, commit: &str) -> bool {
        if !self.current(job) {
            return true;
        }
        if key.pinned(commit) {
            return false;
        }
        let resolved = self
            .reader(job, &key.project)
            .and_then(|reader| lenses::revision(&reader, key));
        if resolved.is_ok_and(|rev| rev.commit() == commit) {
            return false;
        }
        self.publish(job, |cache| cache.requeue_lenses(key));
        true
    }
}

pub(super) fn review_lenses(
    reader: &FactsReader<'_>,
    (base, before): (&Revision, &TreeFacts),
    (head, after): (&Revision, &TreeFacts),
    enrich: bool,
) -> Result<Option<Arc<[LensImpact]>>> {
    let unchanged = || {
        if enrich {
            None
        } else {
            Some(Vec::new().into())
        }
    };
    if base.tree() == head.tree() {
        return Ok(unchanged());
    }
    let old = reader.lens_sources(base)?;
    let new = reader.lens_sources(head)?;
    if old.is_empty() && new.is_empty() {
        return Ok(unchanged());
    }
    let (before, after) = if enrich {
        let mut before = before.clone();
        let mut after = after.clone();
        // Compare enriched definitions only when both snapshot requests succeeded.
        // These are the same tree-keyed answers used for maps and entry impact.
        let Some(old_report) = reader
            .enrich_tree(base, &mut before)
            .filter(|r| r.error.is_none())
        else {
            return Ok(None);
        };
        let Some(new_report) = reader
            .enrich_tree(head, &mut after)
            .filter(|r| r.error.is_none())
        else {
            return Ok(None);
        };
        if !old_report.coverage.provides("definitions")
            && !new_report.coverage.provides("definitions")
        {
            return Ok(None);
        }
        (
            std::borrow::Cow::Owned(before),
            std::borrow::Cow::Owned(after),
        )
    } else {
        (
            std::borrow::Cow::Borrowed(before),
            std::borrow::Cow::Borrowed(after),
        )
    };
    reader
        .lens_impact(base, head, &old, &new, &before, &after)
        .map(|rows| Some(rows.into()))
}
