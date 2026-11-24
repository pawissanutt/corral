use std::{sync::Arc, time::Instant};

use anyhow::{Result, ensure};
use corral_facts::Revision;
use corral_types::{DataSource, DataType, SystemDataRow};

use super::{Job, Worker};
use crate::system::{
    Stage, bounded_error, bounded_report,
    data::{DataKey, RETRY_DELAY},
    provider_row_fits,
};

pub(super) fn check_type(data_type: &DataType) -> Result<()> {
    provider_row_fits(&SystemDataRow::Type {
        data_type: data_type.clone(),
    })
}

impl Worker {
    pub(super) fn data_builtin(&self, job: &Job, key: &DataKey) {
        let result = (|| -> Result<_> {
            let reader = self.reader(job, &key.project)?;
            let rev = reader.resolve_revision(&key.commit)?;
            ensure!(
                rev.commit() == key.commit,
                "selector is not a full Git commit ID"
            );
            let data = reader.builtin_tree(&rev)?.data;
            Ok((rev, data))
        })();
        self.publish(job, move |cache| {
            let revision = cache.stamp();
            let entry = cache.data.get_mut(key)?;
            let old = entry.snapshot.clone();
            entry.snapshot.revision = revision;
            entry.snapshot.building = false;
            match result {
                Ok((rev, data)) => {
                    entry.snapshot.head = Some(rev.commit().to_owned());
                    entry.snapshot.facts = Some(Arc::new(data));
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
        self.publish(job, move |cache| {
            let revision = cache.stamp();
            let entry = cache.data.get_mut(key)?;
            entry.snapshot.revision = revision;
            let old = if report.error.is_none() {
                entry.snapshot.facts.replace(Arc::new(tree.data))
            } else {
                None
            };
            entry.snapshot.provider = Some(Arc::new(report));
            cache.detail_row(&key.project, revision);
            old
        });
    }
}
