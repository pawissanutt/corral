//! One change-feed step of the search index (plan T-253 section 5). Every read comes from the
//! caller's view, the one whose feed position is published afterwards.
//!
//! Work follows what changed, not what accumulated:
//! - a head change re-derives the spec from the head and refreshes the task's titles in place;
//! - comments are append-only, so only comment rows after the last indexed one are read;
//! - an artifact row change arrives with its `artifact-keys` record, so only that row is read
//!   (writers that bypass those records fall back to the task's artifact rows);
//! - events are read after the index's own durable event position;
//! - current container changes and a single demotion use bounded projections and keyed rows;
//!   history edits or multiple demotions re-derive run and plan review documents from rows.
//!   The feed names families, not rows, so history edits still require that fallback.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;

use anyhow::{Context, Result};
use corral_core::backend::{PageReq, TaskPart};
use corral_core::{EventCursor, PinRef, TaskComment};

use super::{
    Cache, DOCUMENT_BYTES, INDEX_BYTES, Record, Source, TEXT_BUDGET, VERSION, extract, lock_index,
    read_cache, update_postings,
};
use crate::Ctx;
use crate::artifact_storage::{ARTIFACT_KEYS, KeyDoc, task_of};

/// What one task's changes touched: its head, and the row families written.
#[derive(Debug, Default)]
pub(crate) struct TaskDelta {
    pub parts: BTreeSet<TaskPart>,
    /// A container's history was edited since the cursor; without one only the current
    /// containers' documents and a demoted one's can have changed.
    pub history: bool,
    /// A current container was demoted since the cursor.
    pub demoted: bool,
}

/// What changed after the index's cursor.
#[derive(Debug, Default)]
pub(crate) struct Delta {
    pub tasks: BTreeMap<String, TaskDelta>,
    /// Paths whose `artifact-keys` record changed: exactly the artifact rows written.
    pub artifacts: BTreeSet<String>,
    pub events: bool,
}

/// Applies `delta` to the index, reading only through `ctx` (bound to the view whose position
/// the caller publishes). Returns false, writing nothing, when there is no current index, the
/// index has no durable event position, or the result exceeds the index byte limit; the caller
/// then rebuilds. A full document or text budget evicts documents instead, since a rebuild
/// fills both budgets and could never be followed again.
pub(crate) fn follow(ctx: &Ctx, delta: &Delta) -> Result<bool> {
    let dir = ctx.store.cache_root()?;
    let _lock = lock_index(&dir)?;
    let path = dir.join("content-search.json");
    let previous = match File::open(&path) {
        Ok(file)
            if file
                .metadata()
                .context("reading search index metadata")?
                .len()
                <= INDEX_BYTES =>
        {
            read_cache(file)
        }
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("opening search index"),
    };
    let Some(mut events_after) = previous
        .events_after
        .filter(|_| previous.version == VERSION)
    else {
        return Ok(false);
    };
    let mut documents = previous.documents.clone();
    let mut records = Vec::new();
    for (id, changed) in &delta.tasks {
        task(ctx, id, changed, delta, &mut documents, &mut records)?;
    }
    if delta.events {
        let mut reader = ctx.store.events_since(Some(EventCursor {
            seq: events_after,
            ..EventCursor::default()
        }))?;
        loop {
            let before = reader.read_position();
            match reader.next_event_at()? {
                Some((_, event, _)) => {
                    let (document, detail) = super::events::document(event);
                    records.push(Record {
                        document,
                        source: Source::Text(detail),
                    });
                }
                None if reader.read_position() == before => break,
                None => {}
            }
        }
        events_after = events_after.max(reader.cursor().seq);
    }
    for mut record in records {
        if let Source::Artifact(artifact) = &record.source {
            record.document.file_stamp = Some(crate::content_refresh::file_stamp(
                &ctx.store.dir().join(&artifact.path),
            )?);
        }
        match previous
            .documents
            .get(&record.document.reference)
            .filter(|c| {
                c.fingerprint == record.document.fingerprint
                    && c.file_stamp == record.document.file_stamp
            }) {
            Some(cached) => {
                record.document.text = cached.text.clone();
                record.document.complete = cached.complete;
                record.document.binary = cached.binary;
            }
            None => extract(ctx, &mut record)?,
        }
        documents.insert(record.document.reference.clone(), record.document);
    }
    // Evicted documents stay in the snapshot and are streamed during search.
    let mut text: usize = documents.values().map(|d| d.text.len()).sum();
    while documents.len() > 4096 || text > TEXT_BUDGET.saturating_sub(DOCUMENT_BYTES) {
        let Some((_, evicted)) = documents.pop_first() else {
            break;
        };
        text -= evicted.text.len();
    }
    let mut cache: Cache = previous;
    update_postings(&mut cache, &documents);
    cache.documents = documents;
    cache.events_after = Some(events_after);
    let bytes = serde_json::to_vec(&cache).context("serializing search index")?;
    if u64::try_from(bytes.len()).context("index size overflow")? > INDEX_BYTES {
        return Ok(false);
    }
    crate::artifact_storage::replace(&path, &bytes)?;
    Ok(true)
}

/// Re-derives the documents one task's changes touched.
pub(super) fn task(
    ctx: &Ctx,
    id: &str,
    changed: &TaskDelta,
    delta: &Delta,
    documents: &mut BTreeMap<String, super::Document>,
    records: &mut Vec<Record>,
) -> Result<()> {
    let head = ctx.store.head(id)?;
    let fields = &head.task;
    let ident = (fields.id.as_str(), fields.title.as_str(), fields.updated);
    let spec = format!("{id}:spec");
    let comment = format!("{id}:comment:");
    for (reference, doc) in documents.iter_mut().filter(|(_, d)| d.task == id) {
        if doc.scope == "artifacts" || reference.starts_with("events:") {
            continue;
        }
        doc.title = fields.title.clone();
        if reference.starts_with(&comment) {
            doc.updated = fields.updated;
        }
    }
    documents.remove(&spec);
    records.push(Record::text_of(
        ident,
        None,
        "specs",
        "spec",
        fields.body.clone(),
    ));
    if changed.parts.contains(&TaskPart::Comment) {
        let indexed = documents.keys().filter(|r| r.starts_with(&comment)).count();
        let mut page = PageReq {
            after: (indexed > 0).then(|| corral_core::store::codec::ordinal_key(indexed)),
            limit: 256,
        };
        let rows = ctx
            .store
            .task_rows(id, TaskPart::Comment, None, None, &mut page)?;
        for (i, row) in rows.into_iter().enumerate() {
            let comment: TaskComment = serde_json::from_value(row).context("decoding comment")?;
            let key = format!("comment:{}", indexed + i);
            records.push(Record::text_of(ident, None, "reviews", &key, comment.text));
        }
    }
    if [
        TaskPart::Run,
        TaskPart::ReviewRun,
        TaskPart::Plan,
        TaskPart::PlanReview,
    ]
    .iter()
    .any(|part| changed.parts.contains(part))
        && !changed.history
        && !(changed.demoted
            && (changed.parts.contains(&TaskPart::Plan)
                || changed.parts.contains(&TaskPart::PlanReview)))
    {
        current_records(ident, &head, documents, records);
        // A run demoted since the cursor keeps its last state: only it, the run before the
        // current one, and its last child can have changed.
        if changed.demoted
            && let Some(previous) = corral_core::TaskRef::previous_run_head(&head)?
        {
            documents.remove(&format!("{id}:{}:summary", previous.id));
            documents.remove(&format!("{id}:{}:review", previous.id));
            run_records(ident, &previous, &previous.id, records);
            // Older child summaries use the task timestamp, just as re-derivation does;
            // review documents instead use the review's own timestamp.
            let prefix = format!("{id}:{}:reviewer:", previous.id);
            for (key, document) in documents.iter_mut() {
                if key.starts_with(&prefix) && key.ends_with(":summary") {
                    document.updated = fields.updated;
                }
            }
            // Older children cannot change without a history edit (handled below).
            // Only the last child can have changed while this run was current.
            if let Some(child) = head.latest_review_child(&previous.id)? {
                let i = previous.facts.children.saturating_sub(1);
                let key = format!("{}:reviewer:{i}", previous.id);
                documents.remove(&format!("{id}:{key}:summary"));
                documents.remove(&format!("{id}:{key}:review"));
                run_records(ident, &child, &key, records);
            }
        }
    } else if [
        TaskPart::Run,
        TaskPart::ReviewRun,
        TaskPart::Plan,
        TaskPart::PlanReview,
    ]
    .iter()
    .any(|part| changed.parts.contains(part))
    {
        // A demotion or history edit: every run and plan review document is re-derived from
        // rows, so a container demoted since the cursor is never missed.
        documents
            .retain(|r, doc| doc.task != id || doc.scope != "reviews" || r.starts_with(&comment));
        let mut page = PageReq::first(256);
        for doc in ctx
            .store
            .task_rows(id, TaskPart::Run, None, None, &mut page)?
        {
            let run: corral_core::RunHead = serde_json::from_value(doc).context("decoding run")?;
            run_records(ident, &run, &run.id, records);
            for (i, child) in head.review_children(&run.id)?.iter().enumerate() {
                run_records(ident, child, &format!("{}:reviewer:{i}", run.id), records);
            }
        }
        let mut page = PageReq::first(256);
        for doc in ctx
            .store
            .task_rows(id, TaskPart::Plan, None, None, &mut page)?
        {
            let plan = doc["id"]
                .as_str()
                .context("plan row without id")?
                .to_string();
            for review in head.plan_reviews(&plan)? {
                records.push(plan_review_record(ident, &plan, &review));
            }
        }
    }
    if changed.parts.contains(&TaskPart::Artifact) {
        let keyed: Vec<&String> = delta
            .artifacts
            .iter()
            .filter(|path| task_of(path) == Some(id))
            .collect();
        if keyed.is_empty() {
            // A writer that bypasses the keyed records: re-derive the task's artifacts.
            documents.retain(|_, doc| doc.task != id || doc.scope != "artifacts");
            for artifact in ctx.store.artifact_rows(id, None)? {
                artifact_record(ctx, id, artifact, records)?;
            }
        }
        for path in keyed {
            let Some(key) = ctx.store.record::<KeyDoc>(ARTIFACT_KEYS, path)? else {
                continue;
            };
            let Some(artifact) = ctx.store.artifact_row(&key.task, &key.row)? else {
                continue;
            };
            documents.remove(&PinRef::of_artifact(id, &artifact).to_string());
            artifact_record(ctx, id, artifact, records)?;
        }
    }
    Ok(())
}

fn artifact_record(
    ctx: &Ctx,
    id: &str,
    artifact: corral_core::Artifact,
    records: &mut Vec<Record>,
) -> Result<()> {
    if artifact.purged_at.is_none() && crate::artifact_storage::content_available(ctx, &artifact)? {
        records.push(Record::artifact(id, &artifact));
    }
    Ok(())
}

/// Only current containers changed: their documents are re-derived from the head's
/// projections (the current run, its current child and the current plan review).
fn current_records(
    ident: (&str, &str, jiff::Timestamp),
    head: &corral_core::TaskHead,
    documents: &mut BTreeMap<String, super::Document>,
    records: &mut Vec<Record>,
) {
    let id = ident.0;
    let mut rederive = |run: &corral_core::RunHead, key: &str, records: &mut Vec<Record>| {
        documents.remove(&format!("{id}:{key}:summary"));
        documents.remove(&format!("{id}:{key}:review"));
        run_records(ident, run, key, records);
    };
    if let Some(run) = &head.current_run {
        rederive(run, &run.id, records);
        if let Some(child) = run.current_child.as_deref() {
            let ordinal = run.facts.children.saturating_sub(1);
            rederive(child, &format!("{}:reviewer:{ordinal}", run.id), records);
        }
    }
    if let Some(plan) = &head.last_plan
        && let Some(review) = &plan.current_review
    {
        records.push(plan_review_record(ident, &plan.id, review));
    }
}

fn plan_review_record(
    ident: (&str, &str, jiff::Timestamp),
    plan: &str,
    review: &corral_core::PlanReviewHead,
) -> Record {
    Record::text_of(
        ident,
        None,
        "reviews",
        &format!("{plan}:{}", review.id),
        format!(
            "Verdict: {}\n{}",
            review.verdict.map_or("", |v| v.as_str()),
            review.notes.as_deref().unwrap_or("")
        ),
    )
}

/// The summary and review documents of `run` under `key`, as `content_index::runs` derives
/// them from a full run, without its nested children.
fn run_records(
    ident: (&str, &str, jiff::Timestamp),
    run: &corral_core::RunHead,
    key: &str,
    records: &mut Vec<Record>,
) {
    if let Some(summary) = &run.summary {
        let key = format!("{key}:summary");
        records.push(Record::text_of(
            ident,
            Some(&run.id),
            "reviews",
            &key,
            summary.clone(),
        ));
    }
    if let Some(review) = &run.review {
        let text = format!(
            "Verdict: {}\n{}",
            review.verdict.map_or("", |v| v.as_str()),
            review.notes.as_deref().unwrap_or("")
        );
        let mut record = Record::text_of(
            ident,
            Some(&run.id),
            "reviews",
            &format!("{key}:review"),
            text,
        );
        record.document.updated = review.at;
        records.push(record);
    }
}
