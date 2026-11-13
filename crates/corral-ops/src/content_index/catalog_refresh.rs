//! Durable source batches. Serving queries never acquire the private builder lease.
use std::collections::{BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::backend::{
    ChangeScope, ChangeSeq, EventQuery, EventSeq, FeedError, PageReq, TaskOrder, TaskQuery,
};
use serde::{Deserialize, Serialize};

use super::{
    Catalog, Entry, IndexedSource, Manifest, load_prefix_cache, metadata, metadata_entry,
    write_record,
};
use crate::Ctx;
use crate::content_index::{Record, Source};

#[path = "catalog_worker.rs"]
mod worker;
pub(super) use worker::schedule;

const VERSION: u32 = 2;
const ROWS: u32 = 16;
const FEED_ENTRIES: u32 = 64;

#[derive(Serialize, Deserialize)]
enum Phase {
    Headers(Option<String>),
    Tasks,
    Events,
    Follow,
    Publish,
}

#[derive(Serialize, Deserialize)]
struct PendingMeta {
    id: String,
    scan: metadata::TaskScan,
    input: String,
    bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct PendingIndex {
    id: Option<String>,
    input: String,
    offset: u64,
    output: String,
    bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct Progress {
    manifest: Manifest,
    phase: Phase,
    tasks: VecDeque<String>,
    assistants: bool,
    target: u64,
    event_stop: Option<u64>,
    meta: Option<PendingMeta>,
    index: Option<PendingIndex>,
    reindex: bool,
    force_sources: bool,
}

pub(super) enum Step {
    More,
    Busy,
    Ready,
}

#[cfg(test)]
pub(crate) fn advance_batch(ctx: &Ctx) -> Result<bool> {
    Ok(matches!(advance(ctx)?, Step::Ready))
}

pub(super) fn root(ctx: &Ctx) -> Result<PathBuf> {
    Ok(ctx.store.cache_root()?.join("search-catalog"))
}

pub(super) fn request_reindex(ctx: &Ctx) -> Result<()> {
    let root = root(ctx)?;
    fs::create_dir_all(root.join("objects")).context("creating catalog object cache")?;
    crate::artifact_storage::replace(&root.join("restart"), b"canonical")
}

pub(super) fn read_manifest(root: &Path) -> Result<Option<Manifest>> {
    match File::open(root.join("manifest.json")) {
        Ok(file) => Ok(serde_json::from_reader::<_, Manifest>(BufReader::new(file))
            .ok()
            .filter(|manifest| manifest.version == VERSION)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("opening published catalog"),
    }
}

fn relevant(scope: &ChangeScope) -> bool {
    matches!(
        scope,
        ChangeScope::Task { .. } | ChangeScope::Artifact { .. } | ChangeScope::Events { .. }
    ) || matches!(scope, ChangeScope::Record { area, .. } if area == "assistant-sessions")
}

pub(super) fn current(ctx: &Ctx, after: u64) -> Result<bool> {
    let view = ctx.store.view()?;
    if view.seq().0 == after {
        return Ok(true);
    }
    let page = match view.changes(ChangeSeq(after), FEED_ENTRIES) {
        Ok(page) => page,
        Err(FeedError::Gap { .. }) => return Ok(false),
        Err(error) => return Err(error).context("checking catalog freshness"),
    };
    Ok(!page.more
        && !page
            .entries
            .iter()
            .flat_map(|entry| &entry.scopes)
            .any(relevant))
}

pub(super) fn published(ctx: &Ctx) -> Result<Option<Catalog>> {
    let root = root(ctx)?;
    if root.join("restart").is_file() {
        return Ok(None);
    }
    let Some(manifest) = read_manifest(&root)? else {
        return Ok(None);
    };
    if !current(ctx, manifest.after)? {
        return Ok(None);
    }
    let path = root.join(&manifest.prefix_file);
    let cache = match File::open(path) {
        Ok(file)
            if file
                .metadata()
                .context("inspecting catalog prefix cache")?
                .len()
                <= super::INDEX_BYTES =>
        {
            super::super::read_cache(file)
        }
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("opening catalog prefixes"),
    };
    if cache.version != super::VERSION {
        return Ok(None);
    }
    Ok(Some(Catalog {
        objects: root.join("objects"),
        root,
        manifest,
        cache,
        temporary: None,
    }))
}

fn initialize(ctx: &Ctx, root: &Path, canonical: bool) -> Result<Progress> {
    if !canonical && let Some(manifest) = read_manifest(root)? {
        return Ok(Progress {
            target: manifest.after,
            manifest,
            phase: Phase::Follow,
            tasks: VecDeque::new(),
            assistants: false,
            event_stop: None,
            meta: None,
            index: None,
            reindex: false,
            force_sources: false,
        });
    }
    let after = ctx.store.view()?.seq().0;
    Ok(Progress {
        manifest: Manifest {
            version: VERSION,
            after,
            ..Manifest::default()
        },
        phase: Phase::Headers(None),
        tasks: VecDeque::new(),
        assistants: true,
        target: after,
        event_stop: None,
        meta: None,
        index: None,
        reindex: canonical,
        force_sources: canonical,
    })
}

#[cfg(test)]
pub(super) fn advance(ctx: &Ctx) -> Result<Step> {
    advance_with_cache(ctx, None)
}

pub(super) fn advance_with_cache(ctx: &Ctx, cache: Option<&super::Cache>) -> Result<Step> {
    let _refresh = corral_core::timing::span("search.index_refresh");
    let _span = corral_core::timing::span("search.catalog.batch");
    let root = root(ctx)?;
    fs::create_dir_all(root.join("objects")).context("creating catalog batch directory")?;
    let lease = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("builder.lock"))
        .context("opening catalog builder lease")?;
    match lease.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => return Ok(Step::Busy),
        Err(fs::TryLockError::Error(error)) => {
            return Err(error).context("taking catalog builder lease");
        }
    }
    let restart = root.join("restart").is_file();
    let path = root.join("progress.json");
    let mut progress = if restart {
        initialize(ctx, &root, true)?
    } else {
        match File::open(&path) {
            Ok(file) => match serde_json::from_reader(BufReader::new(file)) {
                Ok(progress) => progress,
                Err(_) => initialize(ctx, &root, false)?,
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                initialize(ctx, &root, false)?
            }
            Err(error) => return Err(error).context("opening catalog progress"),
        }
    };
    // A restart request is consumed only after the new starting checkpoint is durable.
    if restart {
        save(&path, &progress)?;
        fs::remove_file(root.join("restart")).context("consuming catalog restart request")?;
    }
    let ready = if progress.meta.is_some() {
        metadata_batch(ctx, &root, &mut progress)?;
        false
    } else if progress.index.is_some() {
        index_batch(ctx, &root, &mut progress, cache)?;
        false
    } else {
        match &progress.phase {
            Phase::Headers(_) => {
                header_batch(ctx, &mut progress)?;
                false
            }
            Phase::Tasks => {
                task_batch(ctx, &root, &mut progress)?;
                false
            }
            Phase::Events => {
                event_batch(ctx, &root, &mut progress)?;
                false
            }
            Phase::Follow => {
                follow_batch(ctx, &mut progress)?;
                false
            }
            Phase::Publish => {
                if !current(ctx, progress.manifest.after)? {
                    progress.phase = Phase::Follow;
                    false
                } else {
                    let mut catalog = Catalog {
                        objects: root.join("objects"),
                        root: root.clone(),
                        manifest: progress.manifest.clone(),
                        cache: load_prefix_cache(ctx)?,
                        temporary: None,
                    };
                    // No store view or artifact-storage/index lock survives into admission.
                    catalog.rebuild_prefixes(ctx, progress.reindex, false)?;
                    let bytes = serde_json::to_vec(&catalog.manifest)
                        .context("encoding published catalog")?;
                    crate::artifact_storage::replace(&root.join("manifest.json"), &bytes)?;
                    progress.manifest = catalog.manifest;
                    true
                }
            }
        }
    };
    save(&path, &progress)?;
    #[cfg(test)]
    after_checkpoint()?;
    if ready {
        fs::remove_file(&path).context("retiring completed catalog progress")?;
        Ok(Step::Ready)
    } else {
        Ok(Step::More)
    }
}

fn save(path: &Path, progress: &Progress) -> Result<()> {
    crate::artifact_storage::replace(
        path,
        &serde_json::to_vec(progress).context("encoding catalog progress")?,
    )
}

fn new_partition(root: &Path) -> Result<String> {
    let temp = tempfile::Builder::new()
        .prefix("source-")
        .suffix(".jsonl")
        .tempfile_in(root)
        .context("creating immutable catalog partition")?;
    let (_, path) = temp.keep().context("retaining catalog partition")?;
    Ok(path
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid catalog filename")?
        .to_string())
}

fn append(root: &Path, path: &str, bytes: u64) -> Result<File> {
    let mut file = OpenOptions::new()
        .write(true)
        .open(root.join(path))
        .context("opening catalog batch output")?;
    file.set_len(bytes)
        .context("discarding unpublished catalog suffix")?;
    file.seek(SeekFrom::Start(bytes))
        .context("positioning catalog batch output")?;
    Ok(file)
}

fn append_metadata(root: &Path, path: &str, bytes: u64, records: Vec<Record>) -> Result<u64> {
    let mut writer = BufWriter::new(append(root, path, bytes)?);
    for record in records {
        let entry = metadata_entry(root, record)?;
        if let IndexedSource::Text(key) = &entry.source {
            File::open(root.join("objects").join(format!("{key}.body")))
                .context("opening checkpoint text")?
                .sync_all()
                .context("syncing checkpoint text")?;
        }
        serde_json::to_writer(&mut writer, &entry).context("writing source batch metadata")?;
        writer
            .write_all(b"\n")
            .context("terminating source batch metadata")?;
    }
    writer.flush().context("flushing source metadata batch")?;
    writer
        .get_ref()
        .sync_all()
        .context("syncing source metadata batch")?;
    writer
        .stream_position()
        .context("reading source metadata checkpoint")
}

fn header_batch(ctx: &Ctx, progress: &mut Progress) -> Result<()> {
    let Phase::Headers(after) = &progress.phase else {
        return Ok(());
    };
    let page = {
        let view = ctx.store.view()?;
        let query = TaskQuery {
            statuses: Vec::new(),
            project: None,
            order: TaskOrder::IdAsc,
        };
        view.tasks(
            &query,
            PageReq {
                after: after.clone(),
                limit: FEED_ENTRIES,
            },
        )
        .context("reading catalog task header batch")?
    };
    progress
        .tasks
        .extend(page.items.into_iter().map(|header| header.id));
    progress.phase = match page.next {
        Some(after) => Phase::Headers(Some(after)),
        None => Phase::Tasks,
    };
    Ok(())
}

fn task_batch(ctx: &Ctx, root: &Path, progress: &mut Progress) -> Result<()> {
    if let Some(id) = progress.tasks.pop_front() {
        if let Some((scan, spec)) = metadata::begin(ctx, &id)? {
            let input = new_partition(root)?;
            let bytes = append_metadata(root, &input, 0, vec![spec])?;
            progress.meta = Some(PendingMeta {
                id,
                scan,
                input,
                bytes,
            });
        } else {
            progress.manifest.tasks.remove(&id);
        }
    } else if progress.assistants {
        // One assistant record is the smallest core API; no task histories are read.
        let sessions = ctx.store.list_assistants()?;
        let records = sessions
            .into_iter()
            .flat_map(|session| {
                let id = session.id;
                session
                    .artifacts
                    .into_iter()
                    .filter(|artifact| artifact.purged_at.is_none())
                    .map(move |artifact| Record::artifact(&id, &artifact))
            })
            .collect();
        let input = new_partition(root)?;
        append_metadata(root, &input, 0, records)?;
        progress.index = Some(PendingIndex {
            id: None,
            input,
            offset: 0,
            output: new_partition(root)?,
            bytes: 0,
        });
        progress.assistants = false;
    } else {
        progress.phase = Phase::Events;
    }
    Ok(())
}

fn metadata_batch(ctx: &Ctx, root: &Path, progress: &mut Progress) -> Result<()> {
    let pending = progress
        .meta
        .as_mut()
        .context("missing task metadata progress")?;
    let (records, done) = metadata::next(ctx, &mut pending.scan)?;
    pending.bytes = append_metadata(root, &pending.input, pending.bytes, records)?;
    if done {
        let pending = progress
            .meta
            .take()
            .context("missing completed task metadata")?;
        progress.index = Some(PendingIndex {
            id: Some(pending.id),
            input: pending.input,
            offset: 0,
            output: new_partition(root)?,
            bytes: 0,
        });
    }
    Ok(())
}

fn index_batch(
    ctx: &Ctx,
    root: &Path,
    progress: &mut Progress,
    cache: Option<&super::Cache>,
) -> Result<()> {
    let pending = progress
        .index
        .as_mut()
        .context("missing source index progress")?;
    let mut reader = BufReader::new(
        File::open(root.join(&pending.input)).context("opening queued source metadata")?,
    );
    reader
        .seek(SeekFrom::Start(pending.offset))
        .context("resuming source metadata")?;
    let mut line = String::new();
    if reader
        .read_line(&mut line)
        .context("reading queued source metadata")?
        == 0
    {
        let pending = progress
            .index
            .take()
            .context("missing completed source index")?;
        if let Some(id) = pending.id {
            progress.manifest.tasks.insert(id, pending.output);
        } else {
            progress.manifest.assistants = Some(pending.output);
        }
        return Ok(());
    }
    let entry: Entry = serde_json::from_str(&line).context("decoding queued source metadata")?;
    let source = match entry.source {
        IndexedSource::Artifact(artifact) => Source::Artifact(artifact),
        IndexedSource::Text(key) => Source::File(root.join("objects").join(format!("{key}.body"))),
        IndexedSource::Event(seq) => Source::Text(super::event_text(ctx, seq)?),
        IndexedSource::Slice { .. } => anyhow::bail!("temporary source in durable source queue"),
    };
    let owned_cache;
    let cache = match cache {
        Some(cache) => cache,
        None => {
            owned_cache = load_prefix_cache(ctx)?;
            &owned_cache
        }
    };
    let cached = cache.documents.get(&entry.document.reference);
    let mut writer = BufWriter::new(append(root, &pending.output, pending.bytes)?);
    // Exactly one source document per lexical batch. Its reader outlives no ReadView or
    // artifact-storage lock; an oversized archive never blocks a serving query.
    write_record(
        ctx,
        &root.join("objects"),
        Record {
            document: entry.document,
            source,
        },
        &mut writer,
        progress.force_sources,
        cached,
    )?;
    writer.flush().context("flushing lexical source batch")?;
    writer
        .get_ref()
        .sync_all()
        .context("syncing lexical source batch")?;
    pending.bytes = writer
        .stream_position()
        .context("reading lexical source checkpoint")?;
    pending.offset += line.len() as u64;
    Ok(())
}

fn event_batch(ctx: &Ctx, root: &Path, progress: &mut Progress) -> Result<()> {
    let (stop, rows) = {
        let view = ctx.store.view()?;
        let stop = match progress.event_stop {
            Some(stop) => stop,
            None => view
                .events(
                    &EventQuery {
                        newest_first: true,
                        ..EventQuery::default()
                    },
                    PageReq::first(1),
                )
                .context("reading catalog event tail")?
                .items
                .first()
                .map_or(0, |row| row.seq.0),
        };
        let rows = view
            .events(
                &EventQuery {
                    after: Some(EventSeq(progress.manifest.events_after)),
                    ..EventQuery::default()
                },
                PageReq::first(ROWS),
            )
            .context("reading catalog event batch")?
            .items;
        (stop, rows)
    };
    progress.event_stop = Some(stop);
    if rows.is_empty() || progress.manifest.events_after >= stop {
        progress.event_stop = None;
        progress.manifest.after = progress.target;
        progress.phase = Phase::Follow;
        progress.force_sources = false;
        return Ok(());
    }
    let path = new_partition(root)?;
    let mut writer = BufWriter::new(append(root, &path, 0)?);
    for row in rows.into_iter().take_while(|row| row.seq.0 <= stop) {
        let seq = row.seq.0;
        if let Some((document, detail)) = crate::content_index::events::stored_document(row)? {
            write_record(
                ctx,
                &root.join("objects"),
                Record {
                    document,
                    source: Source::Text(detail),
                },
                &mut writer,
                progress.force_sources,
                None,
            )?;
        }
        progress.manifest.events_after = seq;
    }
    writer.flush().context("flushing catalog event batch")?;
    writer
        .get_ref()
        .sync_all()
        .context("syncing catalog event batch")?;
    progress.manifest.events.push(path);
    Ok(())
}

fn follow_batch(ctx: &Ctx, progress: &mut Progress) -> Result<()> {
    let (seq, page) = {
        let view = ctx.store.view()?;
        (
            view.seq().0,
            view.changes(ChangeSeq(progress.manifest.after), FEED_ENTRIES),
        )
    };
    let page = match page {
        Ok(page) => page,
        Err(FeedError::Gap { .. }) => {
            *progress = initialize(ctx, &root(ctx)?, true)?;
            return Ok(());
        }
        Err(error) => return Err(error).context("following catalog change batch"),
    };
    if page.entries.is_empty() {
        progress.manifest.after = seq;
        progress.phase = Phase::Publish;
        return Ok(());
    }
    let mut tasks = BTreeSet::new();
    let mut touched = false;
    for entry in page.entries {
        progress.target = entry.seq.0;
        for scope in entry.scopes {
            match scope {
                ChangeScope::Task { num, .. } => {
                    tasks.insert(corral_core::Store::normalize_id(&num.0.to_string())?);
                    touched = true;
                }
                ChangeScope::Artifact { task } if task.starts_with("T-") => {
                    tasks.insert(task);
                    touched = true;
                }
                ChangeScope::Artifact { .. } => {
                    progress.assistants = true;
                    touched = true;
                }
                ChangeScope::Record { area, .. } if area == "assistant-sessions" => {
                    progress.assistants = true;
                    touched = true;
                }
                ChangeScope::Events { .. } => touched = true,
                _ => {}
            }
        }
    }
    progress.tasks.extend(tasks);
    if touched {
        progress.phase = Phase::Tasks;
    } else {
        progress.manifest.after = progress.target;
    }
    Ok(())
}

#[cfg(test)]
thread_local! { static INTERRUPT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }

#[cfg(test)]
pub(crate) fn interrupt_after_batches(n: usize) {
    INTERRUPT.set(Some(n));
}

#[cfg(test)]
fn after_checkpoint() -> Result<()> {
    match INTERRUPT.get() {
        Some(0 | 1) => {
            INTERRUPT.set(None);
            anyhow::bail!("injected interruption after catalog checkpoint");
        }
        Some(n) => INTERRUPT.set(Some(n - 1)),
        None => {}
    }
    Ok(())
}
