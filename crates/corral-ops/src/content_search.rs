//! Agent-oriented ranked content hits, with bounded snippets and read-only sources.
use std::collections::{BTreeSet, HashMap};

use anyhow::{Result, ensure};
use corral_core::query::{Query, SearchContext, cutoff};
use corral_core::{ArtifactKind, ArtifactTier, Store, Task};
use jiff::Timestamp;
use serde::Serialize;

use crate::Ctx;
use crate::content_index::{self, Snapshot};

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

fn workspace_task(now: Timestamp) -> Result<Task> {
    serde_json::from_value(serde_json::json!({
        "id": "", "title": "Workspace events", "status": "backlog",
        "created": now, "updated": now,
    }))
    .map_err(Into::into)
}

fn candidates(snapshot: &Snapshot, query: &Query) -> BTreeSet<String> {
    let mut result: Option<BTreeSet<String>> = None;
    for (term, _, negative) in query.content_terms() {
        if negative {
            continue;
        }
        for word in content_index::words(term) {
            let found = snapshot
                .postings
                .iter()
                .filter(|(token, _)| token.contains(&word))
                .flat_map(|(_, ids)| ids.iter().cloned())
                .collect::<BTreeSet<_>>();
            result = Some(match result {
                None => found,
                Some(previous) => previous.intersection(&found).cloned().collect(),
            });
        }
    }
    result.unwrap_or_else(|| {
        snapshot
            .records
            .iter()
            .map(|r| r.document.reference.clone())
            .collect()
    })
}

/// Searches the shared extracted-text index, streaming overflow documents when necessary.
/// The index is a derived cache; tasks, artifact snapshots and events are never modified.
pub fn search_content(ctx: &Ctx, input: &str, options: &ContentSearch) -> Result<Vec<ContentHit>> {
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
    let now = Timestamp::now();
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
    let tasks = ctx.store.artifact_owners()?;
    let by_id: HashMap<_, _> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    let _storage = crate::artifact_storage::storage_lock(ctx)?;
    let snapshot = content_index::snapshot(ctx, &tasks, options.reindex)?;
    let candidates = candidates(&snapshot, &query);
    let pins: BTreeSet<_> = ctx
        .store
        .pins()?
        .into_iter()
        .filter_map(|p| corral_core::PinRef::parse(&p.target).ok())
        .map(|p| p.task().to_string())
        .collect();
    let fallback = workspace_task(now)?;
    let idfs: Vec<u64> = query
        .content_terms()
        .iter()
        .map(|(term, _, _)| {
            let matching = snapshot
                .postings
                .iter()
                .filter(|(word, _)| content_index::words(term).any(|t| word.contains(&t)))
                .flat_map(|(_, ids)| ids.iter())
                .collect::<BTreeSet<_>>()
                .len();
            1 + 1000 / (1 + u64::try_from(matching).unwrap_or(u64::MAX - 1))
        })
        .collect();
    let mut hits = Vec::new();
    for record in &snapshot.records {
        let doc = &record.document;
        let task = by_id.get(doc.task.as_str()).copied().unwrap_or(&fallback);
        if (options.scope != "all" && options.scope != doc.scope)
            || options.task.as_ref().is_some_and(|id| id != &doc.task)
            || options.kind.is_some_and(|kind| kind.as_str() != doc.kind)
            || options.tier.is_some_and(|tier| tier != doc.tier)
            || since.is_some_and(|since| doc.updated < since)
            || options
                .project
                .as_ref()
                .is_some_and(|p| task.project.as_ref() != Some(p))
            || (doc.complete && !candidates.contains(&doc.reference))
        {
            continue;
        }
        if let Some(found) = crate::content_scan::search_record(
            ctx,
            record,
            task,
            &query,
            SearchContext {
                pinned: pins.contains(&doc.task),
                extra_text: "",
            },
            options.context,
            &idfs,
        )? {
            hits.push(ContentHit {
                reference: doc.reference.clone(),
                artifact_ref: (doc.scope == "artifacts").then(|| doc.reference.clone()),
                task: doc.task.clone(),
                run: doc.run.clone(),
                project: task.project.clone(),
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
    }
    Ok(hits)
}

/// Adds indexed content to task-search snapshots without changing their public result shape.
pub(crate) fn task_text(store: &Store, tasks: &[Task]) -> Result<HashMap<String, String>> {
    let ctx = Ctx::from_store(store.clone());
    let _storage = crate::artifact_storage::storage_lock(&ctx)?;
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
