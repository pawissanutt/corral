use std::{sync::Arc, time::Instant};

use anyhow::{Result, ensure};
use corral_engine::facts::FactsReader;
use corral_facts::Revision;
use corral_types::{
    DataChange, DataChangeKind, DataSource, DataType, SourceExcerpt, SystemDataRow, SystemReviewRow,
};

use super::{Job, Worker};
use crate::system::{
    Stage, bounded_error, bounded_report,
    data::{DataKey, HistoricalFacts, HistoricalProjection, RETRY_DELAY},
    provider_row_fits,
};

pub(super) fn check_type(data_type: &DataType) -> Result<()> {
    provider_row_fits(&SystemDataRow::Type {
        data_type: data_type.clone(),
    })
}

fn review_anchor(change: &DataChange) -> (bool, u64) {
    // The boolean selects the base for removals and the head otherwise.
    match &change.change {
        DataChangeKind::TypeAdded => (false, change.line),
        DataChangeKind::TypeRemoved => (true, change.line),
        DataChangeKind::FieldAdded { field } => (false, field.line),
        DataChangeKind::FieldRemoved { field } => (true, field.line),
        DataChangeKind::FieldRetyped { after, .. }
        | DataChangeKind::FieldOptionalityChanged { after, .. } => (false, after.line),
    }
}

pub(super) fn review_excerpts(
    reader: &FactsReader<'_>,
    base: &Revision,
    head: &Revision,
    changes: &[DataChange],
) -> Arc<[Option<SourceExcerpt>]> {
    // Bound the worker's staging vectors across both revisions, even for huge deltas.
    const MAX_ANCHORS: usize = 2048;
    let limit = changes.len().min(MAX_ANCHORS);
    let mut sides = [
        (base, Vec::new(), Vec::new()),
        (head, Vec::new(), Vec::new()),
    ];
    for (index, change) in changes[..limit].iter().enumerate() {
        let (at_base, line) = review_anchor(change);
        let (_, anchors, indices) = &mut sides[usize::from(!at_base)];
        anchors.push((change.path.clone(), line));
        indices.push(index);
    }
    let mut excerpts = vec![None; limit];
    for (rev, anchors, indices) in sides {
        for (index, excerpt) in indices.into_iter().zip(reader.line_excerpts(rev, &anchors)) {
            // Optional source must not make an otherwise pageable row disappear.
            excerpts[index] = excerpt.filter(|excerpt| {
                provider_row_fits(&SystemReviewRow::DataChange {
                    change: changes[index].clone(),
                    excerpt: Some(excerpt.clone()),
                })
                .is_ok()
            });
        }
    }
    excerpts.into()
}

impl Worker {
    pub(super) fn data_builtin(&self, job: &Job, key: &DataKey) {
        let result = (|| -> Result<_> {
            let reader = self.reader(job, &key.project)?;
            let rev = match key.projection {
                HistoricalProjection::Data => reader.resolve_revision(&key.commit)?,
                HistoricalProjection::Diagram => reader.resolve_local_commit(&key.commit)?,
            };
            ensure!(
                rev.commit() == key.commit,
                "selector is not a full Git commit ID"
            );
            let tree = reader.builtin_tree(&rev)?;
            let facts = match key.projection {
                HistoricalProjection::Data => HistoricalFacts::Data(Arc::new(tree.data)),
                HistoricalProjection::Diagram => HistoricalFacts::Diagram(Arc::new(tree)),
            };
            Ok((rev, facts))
        })();
        self.publish(job, move |cache| {
            let revision = cache.stamp();
            let entry = cache.data.get_mut(key)?;
            let old = entry.snapshot.clone();
            entry.snapshot.revision = revision;
            entry.snapshot.building = false;
            match result {
                Ok((rev, facts)) => {
                    entry.snapshot.head = Some(rev.commit().to_owned());
                    entry.snapshot.facts = Some(Arc::new(facts));
                    if job.has_provider(&key.project) {
                        entry.pending = Some(Stage::Enrich);
                        entry.enrich = Some(rev);
                    }
                }
                Err(error) => {
                    entry.retry_at = Some(Instant::now() + RETRY_DELAY);
                    entry.snapshot.error = Some(bounded_error(format!("{error:#}")));
                }
            }
            cache.detail_row(&key.project, revision);
            Some(old)
        });
    }

    pub(super) fn data_enrich(&self, job: &Job, key: &DataKey, rev: &Revision) {
        let cancelled = || self.cancelled(job);
        let Ok(reader) = self.reader(job, &key.project) else {
            return;
        };
        let reader = reader.with_cancellation(&cancelled);
        let (report, facts) = match key.projection {
            HistoricalProjection::Data => {
                let Ok(mut tree) = reader.builtin_tree(rev) else {
                    return;
                };
                let Some(report) = reader.enrich_data_tree(rev, &mut tree) else {
                    return;
                };
                let report = bounded_report(report, || {
                    tree.data
                        .types
                        .iter()
                        .filter(|ty| ty.source == DataSource::Extension)
                        .try_for_each(check_type)
                });
                (report, HistoricalFacts::Data(Arc::new(tree.data)))
            }
            HistoricalProjection::Diagram => {
                let Some(facts) = self
                    .queue
                    .lock()
                    .data
                    .get(key)
                    .and_then(|entry| entry.snapshot.facts.clone())
                else {
                    return;
                };
                let HistoricalFacts::Diagram(tree) = facts.as_ref() else {
                    return;
                };
                let mut tree = (**tree).clone();
                // Reuse the System extension but retain exact commit provenance.
                let Some(report) = reader.enrich_diagram_tree(rev, &mut tree) else {
                    return;
                };
                let report = bounded_report(report, || Ok(()));
                (report, HistoricalFacts::Diagram(Arc::new(tree)))
            }
        };
        self.publish(job, move |cache| {
            let revision = cache.stamp();
            let entry = cache.data.get_mut(key)?;
            entry.snapshot.revision = revision;
            let old = if report.error.is_none() {
                entry.snapshot.facts.replace(Arc::new(facts))
            } else {
                None
            };
            entry.snapshot.provider = Some(Arc::new(report));
            cache.detail_row(&key.project, revision);
            old
        });
    }
}
