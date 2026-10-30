//! Literal finder matching over the shared, bounded content snapshot.
use anyhow::{Context, Result};
use corral_core::Task;
use corral_core::query::{Query, SearchContext};
use jiff::Timestamp;

use crate::content_index::{self, Snapshot, Source};
use crate::{ArtifactRow, Ctx};

/// An artifact finder's content snapshot, refreshed alongside its artifact rows.
/// Complete bodies stay cached; overflow uses the shared bounded streaming scanner.
pub struct ArtifactSearch {
    snapshot: Snapshot,
    tasks: Vec<Task>,
}

impl ArtifactSearch {
    /// Loads the shared extracted-text cache without creating a separate finder index.
    ///
    /// # Errors
    /// Returns an error if tasks, snapshots or the derived cache cannot be read.
    pub fn load(ctx: &Ctx) -> Result<Self> {
        let tasks = ctx.store.artifact_owners()?;
        let _storage = crate::artifact_storage::storage_lock(ctx)?;
        let mut snapshot = content_index::snapshot(ctx, &tasks, false)?;
        snapshot
            .records
            .retain(|record| record.document.scope == "artifacts");
        Ok(Self { snapshot, tasks })
    }

    /// ANDs lowercase literal terms across the row's lowercase metadata and body.
    /// The caller retains control of task, kind, tier, fold and ordering filters.
    ///
    /// # Errors
    /// Returns an error if an overflow snapshot cannot be streamed.
    pub fn matches(
        &self,
        ctx: &Ctx,
        row: &ArtifactRow,
        terms: &[&str],
        metadata: &str,
    ) -> Result<bool> {
        if terms.iter().all(|term| metadata.contains(term)) {
            return Ok(true);
        }
        let Some(record) = self.snapshot.records.iter().find(|record| {
            record.document.task == row.task_id
                && matches!(&record.source, Source::Artifact(a) if a.path == row.artifact.path)
        }) else {
            return Ok(false);
        };
        let text = record.document.text.to_lowercase();
        for term in terms
            .iter()
            .filter(|term| !metadata.contains(**term) && !text.contains(**term))
        {
            if record.document.complete || record.document.binary {
                return Ok(false);
            }
            let task = self
                .tasks
                .iter()
                .find(|task| task.id == row.task_id)
                .context("artifact search task missing from snapshot")?;
            // Finder terms are literal, not query syntax. An extra minus negates
            // a leading-minus literal; invert that result after the shared scan.
            let negative = term.starts_with('-');
            let escaped = term.replace('\\', "\\\\").replace('"', "\\\"");
            let input = format!("\"{}{escaped}\"", if negative { "-" } else { "" });
            let query = Query::parse(&input, Timestamp::now())?;
            let found = crate::content_scan::search_record(
                ctx,
                record,
                task,
                &query,
                SearchContext::default(),
                0,
                &[1],
            )?
            .is_some();
            if found == negative {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
