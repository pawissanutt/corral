//! Agent-oriented ranked content hits, with bounded snippets and read-only sources.
use std::collections::{BTreeSet, HashMap};

use anyhow::{Result, ensure};
use corral_core::query::{ContentOwner, Query, SearchContext, cutoff};
use corral_core::{ArtifactKind, ArtifactTier, Store, Task};
use jiff::Timestamp;
use serde::Serialize;

use crate::Ctx;
use crate::content_index::{self, catalog::Catalog};

/// Scope and filters for content search. `None` scope is handled by the legacy task CLI.
#[derive(Debug, Clone)]
pub struct ContentSearch {
    pub scope: String,
    pub task: Option<String>,
    pub kind: Option<ArtifactKind>,
    pub tier: Option<ArtifactTier>,
    pub since: Option<String>,
    pub project: Option<String>,
    pub limit: usize,
    pub context: usize,
    pub reindex: bool,
}

impl Default for ContentSearch {
    fn default() -> Self {
        Self {
            scope: "all".into(),
            task: None,
            kind: None,
            tier: None,
            since: None,
            project: None,
            limit: 20,
            context: 0,
            reindex: false,
        }
    }
}

/// A stable artifact reference, or a task/event reference for non-artifact content.
#[derive(Debug, Clone, Serialize)]
pub struct ContentHit {
    pub reference: String,
    pub artifact_ref: Option<String>,
    pub task: String,
    pub run: Option<String>,
    pub project: Option<String>,
    pub scope: String,
    pub kind: String,
    pub tier: ArtifactTier,
    pub title: String,
    pub line_start: usize,
    pub line_end: usize,
    pub snippet: String,
    pub score: u64,
    pub updated: Timestamp,
}

/// Searches the shared extracted-text index, streaming overflow documents when necessary.
/// The index is a derived cache; tasks, artifact snapshots and events are never modified.
pub fn search_content(ctx: &Ctx, input: &str, options: &ContentSearch) -> Result<Vec<ContentHit>> {
    match search_content_once(ctx, input, options) {
        Err(error) if crate::content_index::catalog::is_corrupt(&error) => {
            crate::content_index::catalog::repair(ctx)?;
            search_content_once(ctx, input, options)
        }
        result => result,
    }
}

fn search_content_once(ctx: &Ctx, input: &str, options: &ContentSearch) -> Result<Vec<ContentHit>> {
    ensure!(input.len() <= 4096, "search query exceeds 4096 bytes");
    ensure!(options.limit <= 100, "search limit must be at most 100");
    ensure!(options.context <= 20, "search context must be at most 20");
    ensure!(
        matches!(
            options.scope.as_str(),
            "artifacts" | "specs" | "reviews" | "all"
        ),
        "search scope must be artifacts, specs, reviews or all"
    );
    let now = corral_core::clock::now();
    let query = Query::parse(input, now)?;
    ensure!(
        query.content_terms().len() <= 64,
        "search query exceeds 64 text terms"
    );
    let since = options
        .since
        .as_deref()
        .map(|s| cutoff(s, now))
        .transpose()?;
    let _span = corral_core::timing::span("search.content");
    // Index before storage; both refresh and candidate reads use one store view.
    let _index = content_index::lock(ctx)?;
    let _storage = crate::artifact_storage::storage_lock(ctx)?;
    let (view_ctx, _) = content_index::one_view(ctx)?;
    let ctx = &view_ctx;
    let catalog = Catalog::open(ctx, options.reindex)?;
    let pins: BTreeSet<_> = ctx
        .store
        .pins()?
        .into_iter()
        .filter_map(|p| corral_core::PinRef::parse(&p.target).ok())
        .map(|p| p.task().to_string())
        .collect();
    let idfs = catalog.idfs(&query);
    let mut hits = Vec::new();
    // Only the current candidate's owner is retained, never all task histories.
    let mut owner_id = String::new();
    let mut owner: Option<corral_core::ArtifactOwner> = None;
    let mut owner_project = None;
    // Owner-dependent predicates use ':' or '>'; quoted punctuation can merely disable
    // this conservative fast path. Bare `pinned` needs only SearchContext, not a Task.
    let needs_owner = input.contains([':', '>']);
    catalog.visit(&query, |entry| {
        let doc = &entry.document;
        if (options.scope != "all" && options.scope != doc.scope)
            || options.task.as_ref().is_some_and(|id| id != &doc.task)
            || options.kind.is_some_and(|kind| kind.as_str() != doc.kind)
            || options.tier.is_some_and(|tier| tier != doc.tier)
            || since.is_some_and(|since| doc.updated < since)
        {
            return Ok(());
        }
        if owner_id != doc.task {
            owner_project = if doc.task.starts_with("T-") {
                ctx.store
                    .head_if_exists(&doc.task)?
                    .and_then(|head| head.project.clone())
            } else {
                None
            };
            owner = if !needs_owner {
                None
            } else if doc.task.starts_with("T-") {
                ctx.store
                    .head_if_exists(&doc.task)?
                    .map(|_| ctx.store.get(&doc.task).map(Into::into))
                    .transpose()?
            } else if doc.task.starts_with("A-") {
                Some(ctx.store.get_assistant(&doc.task)?.into())
            } else {
                None
            };
            owner_id.clone_from(&doc.task);
        }
        let project = owner_project.as_ref();
        if options.project.as_ref().is_some_and(|p| project != Some(p)) {
            return Ok(());
        }
        let record = catalog.record(ctx, entry)?;
        let _scan = corral_core::timing::span(match doc.scope.as_str() {
            "artifacts" => "search.artifact_scan",
            "specs" => "search.spec_scan",
            "reviews" => "search.review_scan",
            _ => "search.event_scan",
        });
        if let Some(found) = crate::content_scan::search_record(
            ctx,
            &record,
            owner
                .as_ref()
                .map(ContentOwner::from)
                .unwrap_or(ContentOwner::Workspace),
            &query,
            SearchContext {
                pinned: pins.contains(&doc.task),
                extra_text: "",
            },
            options.context,
            &idfs,
        )? {
            let _assembly = corral_core::timing::span("search.result_assembly");
            hits.push(ContentHit {
                reference: doc.reference.clone(),
                artifact_ref: (doc.scope == "artifacts").then(|| doc.reference.clone()),
                task: doc.task.clone(),
                run: doc.run.clone(),
                project: project.cloned(),
                scope: doc.scope.clone(),
                kind: doc.kind.clone(),
                tier: doc.tier,
                title: doc.title.chars().take(240).collect(),
                line_start: found.start,
                line_end: found.end,
                snippet: found.snippet,
                score: found.score,
                updated: doc.updated,
            });
            hits.sort_by(|a, b| {
                b.score
                    .cmp(&a.score)
                    .then_with(|| a.tier.cmp(&b.tier))
                    .then_with(|| b.updated.cmp(&a.updated))
                    .then_with(|| a.reference.cmp(&b.reference))
            });
            hits.truncate(options.limit);
        }
        Ok(())
    })?;
    Ok(hits)
}

/// Adds indexed content to task-search snapshots without changing their public result shape.
pub(crate) fn task_text(store: &Store, tasks: &[Task]) -> Result<HashMap<String, String>> {
    let ctx = Ctx::from_store(store.clone());
    let snapshot = content_index::snapshot(&ctx, tasks, false)?;
    let mut text = HashMap::<String, String>::new();
    for record in snapshot.records {
        if matches!(record.document.scope.as_str(), "specs" | "events") {
            continue;
        }
        let entry = text.entry(record.document.task).or_default();
        entry.push('\n');
        entry.push_str(&record.document.text);
    }
    Ok(text)
}
