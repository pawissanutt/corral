//! Rebuildable extracted-text cache and inverted postings. No database dependency is needed.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufReader, Read};

use anyhow::{Context, Result};
use corral_core::backend::ChangeSeq;
use corral_core::{Artifact, ArtifactTier, PinRef, Run, Task};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Ctx;

mod events;
mod follow;

pub(crate) use follow::{Delta, follow};
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use tests::{ARTIFACT_EXTRACTIONS, SNAPSHOTS};

const VERSION: u32 = 1;
pub(crate) const DOCUMENT_BYTES: usize = 256 * 1024;
const TEXT_BUDGET: usize = 8 * 1024 * 1024;
pub(crate) const INDEX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Document {
    pub reference: String,
    pub task: String,
    pub run: Option<String>,
    pub scope: String,
    pub kind: String,
    pub tier: ArtifactTier,
    pub title: String,
    pub updated: Timestamp,
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_stamp: Option<String>,
    pub text: String,
    pub complete: bool,
    pub binary: bool,
}

pub(crate) enum Source {
    Artifact(Artifact),
    Text(String),
}

pub(crate) struct Record {
    pub document: Document,
    pub source: Source,
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    version: u32,
    documents: BTreeMap<String, Document>,
    postings: BTreeMap<String, Vec<String>>,
    /// The last event sequence whose document the index derived; following the feed reads
    /// only events after it, in any process.
    #[serde(default)]
    events_after: Option<u64>,
}

/// Content snapshots are shared by command search and the board's task search.
pub(crate) struct Snapshot {
    pub records: Vec<Record>,
    pub postings: BTreeMap<String, Vec<String>>,
}

pub(crate) fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
}

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

impl Record {
    fn text(task: &Task, run: Option<&str>, scope: &str, key: &str, text: String) -> Self {
        Self::text_of((&task.id, &task.title, task.updated), run, scope, key, text)
    }

    /// A text document of the task identified by `(id, title, updated)`.
    fn text_of(
        (id, title, updated): (&str, &str, Timestamp),
        run: Option<&str>,
        scope: &str,
        key: &str,
        text: String,
    ) -> Self {
        Self {
            document: Document {
                reference: format!("{id}:{key}"),
                task: id.to_string(),
                run: run.map(str::to_string),
                scope: scope.into(),
                kind: scope.trim_end_matches('s').into(),
                tier: ArtifactTier::Keep,
                title: title.to_string(),
                updated,
                fingerprint: digest(&text),
                file_stamp: None,
                text: String::new(),
                complete: false,
                binary: false,
            },
            source: Source::Text(text),
        }
    }

    fn artifact(task: &str, artifact: &Artifact) -> Self {
        Self {
            document: Document {
                reference: PinRef::of_artifact(task, artifact).to_string(),
                task: task.to_string(),
                run: Some(artifact.run.clone()),
                scope: "artifacts".into(),
                kind: artifact.kind.as_str().into(),
                tier: artifact.tier(),
                title: artifact.title.clone(),
                updated: artifact.added,
                // Retention changes encoding, not the extracted text's identity.
                fingerprint: artifact.sha256.clone(),
                file_stamp: None,
                text: String::new(),
                complete: false,
                binary: false,
            },
            source: Source::Artifact(artifact.clone()),
        }
    }
}

fn runs(task: &Task, run: &Run, key: &str, records: &mut Vec<Record>) {
    if let Some(summary) = &run.summary {
        records.push(Record::text(
            task,
            Some(&run.id),
            "reviews",
            &format!("{key}:summary"),
            summary.clone(),
        ));
    }
    if let Some(review) = &run.review {
        let mut record = Record::text(
            task,
            Some(&run.id),
            "reviews",
            &format!("{key}:review"),
            format!(
                "Verdict: {}\n{}",
                review.verdict.map_or("", |v| v.as_str()),
                review.notes.as_deref().unwrap_or("")
            ),
        );
        record.document.updated = review.at;
        records.push(record);
    }
    for (i, child) in run.review_runs.iter().enumerate() {
        runs(task, child, &format!("{key}:reviewer:{i}"), records);
    }
}

fn task_records(ctx: &Ctx, task: &Task, records: &mut Vec<Record>) -> Result<()> {
    records.push(Record::text(task, None, "specs", "spec", task.body.clone()));
    for (i, comment) in task.comments.iter().enumerate() {
        records.push(Record::text(
            task,
            None,
            "reviews",
            &format!("comment:{i}"),
            comment.text.clone(),
        ));
    }
    for run in &task.runs {
        runs(task, run, &run.id, records);
    }
    for plan in &task.plans {
        for review in &plan.reviews {
            records.push(Record::text(
                task,
                None,
                "reviews",
                &format!("{}:{}", plan.id, review.id),
                format!(
                    "Verdict: {}\n{}",
                    review.verdict.map_or("", |v| v.as_str()),
                    review.notes.as_deref().unwrap_or("")
                ),
            ));
        }
    }
    for artifact in task.artifacts.iter().filter(|a| a.purged_at.is_none()) {
        if crate::artifact_storage::content_available(ctx, artifact)? {
            records.push(Record::artifact(&task.id, artifact));
        }
    }
    Ok(())
}

fn sources(ctx: &Ctx, tasks: &[Task]) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    for task in tasks {
        task_records(ctx, task, &mut records)?;
    }
    for session in ctx.store.list_assistants()? {
        for artifact in session
            .artifacts
            .iter()
            .filter(|artifact| artifact.purged_at.is_none())
        {
            if crate::artifact_storage::content_available(ctx, artifact)? {
                records.push(Record::artifact(&session.id, artifact));
            }
        }
    }
    events::records(&ctx.store, &mut records)?;
    records.sort_by(|a, b| {
        b.document
            .updated
            .cmp(&a.document.updated)
            .then_with(|| a.document.reference.cmp(&b.document.reference))
    });
    Ok(records)
}

fn extract(ctx: &Ctx, record: &mut Record) -> Result<()> {
    let mut bytes = Vec::new();
    match &record.source {
        Source::Text(text) => {
            bytes.extend_from_slice(&text.as_bytes()[..text.len().min(DOCUMENT_BYTES + 1)])
        }
        Source::Artifact(artifact) => {
            if !crate::artifact_storage::content_available(ctx, artifact)? {
                // Imported task metadata can outlive its snapshot file.
                record.document.complete = true;
                return Ok(());
            }
            #[cfg(test)]
            ARTIFACT_EXTRACTIONS.with(|count| count.set(count.get() + 1));
            crate::artifact_storage::artifact_reader(ctx, artifact)?
                .take(u64::try_from(DOCUMENT_BYTES + 1).context("document limit overflow")?)
                .read_to_end(&mut bytes)
                .context("extracting search text")?;
        }
    }
    record.document.complete = bytes.len() <= DOCUMENT_BYTES;
    record.document.binary = bytes.iter().take(8192).any(|b| *b == 0);
    bytes.truncate(DOCUMENT_BYTES);
    // A byte limit may split the final UTF-8 code point.
    record.document.text = if record.document.binary {
        record.document.complete = true;
        String::new()
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    Ok(())
}

fn postings(documents: &BTreeMap<String, Document>) -> BTreeMap<String, Vec<String>> {
    let mut postings: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, doc) in documents {
        let terms: BTreeSet<_> = words(&doc.title).chain(words(&doc.text)).collect();
        for term in terms {
            postings.entry(term).or_default().push(id.clone());
        }
    }
    postings
}

fn update_postings(previous: &mut Cache, documents: &BTreeMap<String, Document>) {
    let unchanged: BTreeSet<_> = documents
        .iter()
        .filter_map(|(id, doc)| {
            previous
                .documents
                .get(id)
                .filter(|old| old.title == doc.title && old.text == doc.text)
                .map(|_| id.clone())
        })
        .collect();
    previous.postings.retain(|_, ids| {
        ids.retain(|id| unchanged.contains(id));
        !ids.is_empty()
    });
    for (id, doc) in documents.iter().filter(|(id, _)| !unchanged.contains(*id)) {
        for word in words(&doc.title)
            .chain(words(&doc.text))
            .collect::<BTreeSet<_>>()
        {
            previous.postings.entry(word).or_default().push(id.clone());
        }
    }
    for ids in previous.postings.values_mut() {
        ids.sort();
    }
}

fn read_cache(reader: impl Read) -> Cache {
    // Corrupt and old caches are rebuildable, never authoritative.
    serde_json::from_reader(BufReader::new(reader)).unwrap_or_default()
}

fn lock_index(dir: &std::path::Path) -> Result<File> {
    fs::create_dir_all(dir).context("creating search state directory")?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("content-search.lock"))
        .context("opening search index lock")?;
    lock.lock().context("locking search index")?;
    Ok(lock)
}

/// Adds one newly attached artifact's document to the index, reading only that artifact: the
/// index follows each attach rather than re-snapshotting every task. Whatever does not fit
/// (a full index or text budget) is left to the next refresh.
pub(crate) fn index_artifact(ctx: &Ctx, task: &str, artifact: &Artifact) -> Result<()> {
    let dir = ctx.store.cache_root()?;
    let _lock = lock_index(&dir)?;
    let path = dir.join("content-search.json");
    let mut cache = match File::open(&path) {
        Ok(file)
            if file
                .metadata()
                .context("reading search index metadata")?
                .len()
                <= INDEX_BYTES =>
        {
            read_cache(file)
        }
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Cache::default(),
        Err(error) => return Err(error).context("opening search index"),
    };
    if cache.version != VERSION {
        cache = Cache {
            version: VERSION,
            ..Cache::default()
        };
    }
    let text: usize = cache.documents.values().map(|d| d.text.len()).sum();
    if cache.documents.len() >= 4096 || text + DOCUMENT_BYTES > TEXT_BUDGET {
        return Ok(());
    }
    let mut record = Record::artifact(task, artifact);
    extract(ctx, &mut record)?;
    let id = record.document.reference.clone();
    if cache.documents.get(&id) == Some(&record.document) {
        return Ok(());
    }
    cache.postings.retain(|_, ids| {
        ids.retain(|other| *other != id);
        !ids.is_empty()
    });
    let terms: BTreeSet<_> = words(&record.document.title)
        .chain(words(&record.document.text))
        .collect();
    for term in terms {
        let ids = cache.postings.entry(term).or_default();
        ids.push(id.clone());
        ids.sort();
    }
    cache.documents.insert(id, record.document);
    let bytes = serde_json::to_vec(&cache).context("serializing search index")?;
    if u64::try_from(bytes.len()).context("index size overflow")? > INDEX_BYTES {
        return Ok(());
    }
    crate::artifact_storage::replace(&path, &bytes)
}

/// Reuses unchanged SHA-identified artifact text even after gzip retention.
/// Oversized/evicted documents remain in the snapshot and are streamed during search.
pub(crate) fn snapshot(ctx: &Ctx, tasks: &[Task], reindex: bool) -> Result<Snapshot> {
    if !reindex {
        return build_snapshot(ctx, tasks, Update::Incremental, None);
    }
    // An explicit reindex derives from one read view and publishes that view's feed position,
    // so a change committed after the caller's listing is never acknowledged unindexed.
    let (view_ctx, seq) = one_view(ctx)?;
    let tasks = view_ctx.store.list()?;
    build_snapshot(&view_ctx, &tasks, Update::Reindex, Some(seq))
}

/// Rebuilds the index from `tasks` (read on `ctx`'s view) and publishes `seq`, the feed position
/// of that view.
pub(crate) fn refresh_snapshot(ctx: &Ctx, tasks: &[Task], seq: ChangeSeq) -> Result<Snapshot> {
    build_snapshot(ctx, tasks, Update::Refresh, Some(seq))
}

/// `ctx` bound to one read view of its store, and that view's feed position.
pub(crate) fn one_view(ctx: &Ctx) -> Result<(Ctx, ChangeSeq)> {
    let store = ctx.store.read_only_snapshot()?;
    let seq = store.view()?.seq();
    Ok((
        Ctx {
            store,
            ..ctx.clone()
        },
        seq,
    ))
}

enum Update {
    Incremental,
    Refresh,
    Reindex,
}

fn build_snapshot(
    ctx: &Ctx,
    tasks: &[Task],
    update: Update,
    cursor: Option<ChangeSeq>,
) -> Result<Snapshot> {
    let (reindex, inspect_artifacts) = match update {
        Update::Incremental => (false, false),
        Update::Refresh => (false, true),
        Update::Reindex => (true, true),
    };
    #[cfg(test)]
    SNAPSHOTS.with(|count| count.set(count.get() + 1));
    let dir = ctx.store.cache_root()?;
    let _lock = lock_index(&dir)?;
    let inputs = inspect_artifacts
        .then(|| crate::content_refresh::fingerprint(ctx, tasks))
        .transpose()?;
    let path = dir.join("content-search.json");
    let mut previous = if !reindex && path.exists() {
        let file = File::open(&path).context("opening search index")?;
        if file
            .metadata()
            .context("reading search index metadata")?
            .len()
            <= INDEX_BYTES
        {
            read_cache(file)
        } else {
            Cache::default()
        }
    } else {
        Cache::default()
    };
    let mut records = sources(ctx, tasks)?;
    let mut documents = BTreeMap::new();
    let mut budget = TEXT_BUDGET;
    for record in &mut records {
        if inspect_artifacts {
            record.document.file_stamp = match &record.source {
                Source::Artifact(artifact) => Some(crate::content_refresh::file_stamp(
                    &ctx.store.dir().join(&artifact.path),
                )?),
                Source::Text(_) => None,
            };
        }
        if let Some(cached) = previous
            .documents
            .get(&record.document.reference)
            .filter(|c| previous.version == VERSION && c.fingerprint == record.document.fingerprint)
            .filter(|c| !inspect_artifacts || c.file_stamp == record.document.file_stamp)
        {
            record.document.file_stamp = cached.file_stamp.clone();
            record.document.text = cached.text.clone();
            record.document.complete = cached.complete;
            record.document.binary = cached.binary;
        } else if budget > 0 && documents.len() < 4096 {
            extract(ctx, record)?;
        }
        if record.document.text.len() <= budget && documents.len() < 4096 {
            budget = budget.saturating_sub(record.document.text.len());
            documents.insert(record.document.reference.clone(), record.document.clone());
        } else {
            record.document.text.clear();
            record.document.complete = false;
        }
    }
    let changed = reindex || previous.version != VERSION || previous.documents != documents;
    if previous.version != VERSION {
        previous = Cache::default();
    }
    update_postings(&mut previous, &documents);
    let events_after = records
        .iter()
        .filter_map(|r| {
            r.document
                .reference
                .strip_prefix("events:")?
                .parse::<u64>()
                .ok()
        })
        .max()
        .or(Some(0));
    let mut cache = Cache {
        version: VERSION,
        postings: previous.postings,
        documents,
        events_after,
    };
    let bytes = loop {
        if !changed {
            break None;
        }
        let bytes = serde_json::to_vec(&cache).context("serializing search index")?;
        if u64::try_from(bytes.len()).context("index size overflow")? <= INDEX_BYTES {
            break Some(bytes);
        }
        if let Some(key) = cache.documents.keys().next().cloned() {
            cache.documents.remove(&key);
            cache.postings = postings(&cache.documents);
        } else {
            anyhow::bail!("search index metadata exceeds size limit");
        }
    };
    for record in &mut records {
        if !cache.documents.contains_key(&record.document.reference) {
            record.document.complete = false;
        }
    }
    if let Some(bytes) = bytes {
        crate::artifact_storage::replace(&path, &bytes)?;
    }
    if let Some(inputs) = inputs {
        crate::content_refresh::record_build(ctx, &inputs)?;
    }
    if let Some(seq) = cursor {
        crate::content_refresh::write_cursor(&dir, seq)?;
    }
    Ok(Snapshot {
        records,
        postings: cache.postings,
    })
}
