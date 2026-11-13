//! Literal finder matching over the shared, bounded content snapshot.
use anyhow::Result;
use corral_core::query::{ContentOwner, Query, SearchContext};

use crate::content_index::{self, Record, Source, catalog::Catalog};
use crate::{ArtifactRow, Ctx};

/// An artifact finder's content snapshot, refreshed alongside its artifact rows.
/// Complete bodies stay cached; overflow uses the shared bounded streaming scanner.
pub struct ArtifactSearch {
    records: Vec<Record>,
    catalog: Catalog,
}

impl ArtifactSearch {
    /// Loads the shared extracted-text cache without creating a separate finder index.
    /// A cold or outdated catalog falls back without waiting for background construction.
    ///
    /// # Errors
    /// Returns an error if tasks, snapshots or the derived cache cannot be read.
    pub fn load(ctx: &Ctx) -> Result<Self> {
        match Self::load_once(ctx) {
            Err(error) if content_index::catalog::is_corrupt(&error) => {
                content_index::catalog::repair(ctx)?;
                Self::load_once(ctx)
            }
            result => result,
        }
    }

    fn load_once(ctx: &Ctx) -> Result<Self> {
        let published = Catalog::open(ctx, false)?;
        let _fallback = published
            .is_none()
            .then(|| corral_core::timing::span("search.fallback"));
        let catalog = match published {
            Some(catalog) => catalog,
            None => Catalog::fallback(ctx, false)?,
        };
        let records = catalog.artifact_records(ctx)?;
        Ok(Self { records, catalog })
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
        let Some(record) = self.records.iter().find(|record| {
            record.document.task == row.task_id
                && matches!(&record.source, Source::Artifact(a) if a.path == row.artifact.path)
        }) else {
            return Ok(false);
        };
        let text = record.document.text.to_lowercase();
        let body_terms: Vec<_> = terms
            .iter()
            .copied()
            .filter(|term| !metadata.contains(term) && !text.contains(term))
            .collect();
        if body_terms.is_empty() {
            return Ok(true);
        }
        if record.document.complete || record.document.binary {
            return Ok(false);
        }
        if !self
            .catalog
            .artifact_may_match(&row.artifact, &body_terms)?
        {
            // Lexical rejection must not conceal a source that can no longer be opened.
            // Opening does not scan the unrelated body; actual candidates use the scanner.
            let _reader = crate::artifact_storage::artifact_reader(ctx, &row.artifact)?;
            return Ok(false);
        }
        for term in body_terms {
            // Finder terms are literal, not query syntax. An extra minus negates
            // a leading-minus literal; invert that result after the shared scan.
            let negative = term.starts_with('-');
            let escaped = term.replace('\\', "\\\\").replace('"', "\\\"");
            let input = format!("\"{}{escaped}\"", if negative { "-" } else { "" });
            let query = Query::parse(&input, corral_core::clock::now())?;
            let found = crate::content_scan::search_record(
                ctx,
                record,
                ContentOwner::Workspace,
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
