//! Exact, query-local overlay. The serving generation never changes in place.
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use corral_core::backend::{ChangeScope, ChangeSeq, EventQuery, FeedError, PageReq};

use super::{
    Catalog, Entry, Manifest, fallback, lexical, load_prefix_cache, metadata_entry, refresh,
};
use crate::Ctx;
use crate::content_index::Record;

pub(super) const MAX_FEED_ENTRIES: usize = 16_384;
pub(super) const MAX_TASKS: usize = 64;
pub(super) const MAX_EVENT_DISTANCE: u64 = 4096;

pub(super) struct Delta {
    pub tasks: BTreeSet<String>,
    pub assistants: bool,
    pub after: u64,
    pub events_after: u64,
    pub events_tail: u64,
    pub first_change: Option<jiff::Timestamp>,
}

impl Delta {
    /// Source units: task partitions, the assistant namespace, and event sequence distance.
    pub fn size(&self) -> usize {
        self.tasks.len()
            + usize::from(self.assistants)
            + usize::try_from(self.events_tail - self.events_after)
                .unwrap_or(usize::MAX - MAX_TASKS - 1)
    }
    pub fn has_changes(&self) -> bool {
        self.size() != 0
    }
    pub fn age_seconds(&self) -> i64 {
        self.first_change.map_or(0, |at| {
            corral_core::clock::now()
                .as_second()
                .saturating_sub(at.as_second())
                .max(0)
        })
    }
}

pub(super) fn config_fingerprint(ctx: &Ctx) -> Result<String> {
    Ok(super::digest(
        &serde_json::to_string(&ctx.store.config)
            .context("encoding search configuration fingerprint")?,
    ))
}

fn canonical_rebuild_in_progress(root: &Path) -> Result<bool> {
    #[derive(serde::Deserialize)]
    struct Checkpoint {
        #[serde(default)]
        reindex: bool,
    }
    match File::open(root.join("progress.json")) {
        Ok(file) => Ok(
            serde_json::from_reader::<_, Checkpoint>(BufReader::new(file))
                .map(|checkpoint| checkpoint.reindex)
                .unwrap_or(true),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("checking canonical catalog rebuild"),
    }
}

/// None means the exact window cannot be represented by this bounded overlay.
/// The caller supplies a pinned context so all feed pages and canonical rows agree.
pub(super) fn plan(ctx: &Ctx, manifest: &Manifest) -> Result<Option<Delta>> {
    let _span = corral_core::timing::span("search.catalog.delta_plan");
    let root = refresh::root(ctx)?;
    if root.join("restart").is_file()
        || canonical_rebuild_in_progress(&root)?
        || manifest.config_fingerprint.as_deref() != Some(config_fingerprint(ctx)?.as_str())
    {
        return Ok(None);
    }
    let view = ctx.store.view()?;
    let after = view.seq().0;
    if manifest.after > after {
        return Ok(None);
    }
    let mut delta = Delta {
        tasks: BTreeSet::new(),
        assistants: false,
        after,
        events_after: manifest.events_after,
        events_tail: manifest.events_after,
        first_change: None,
    };
    let mut cursor = manifest.after;
    let mut read = 0_usize;
    let mut events_changed = false;
    while cursor < after {
        let previous = cursor;
        let page = match view.changes(ChangeSeq(cursor), 64) {
            Ok(page) => page,
            Err(FeedError::Gap { .. }) => return Ok(None),
            Err(error) => return Err(error).context("reading exact search delta"),
        };
        read += page.entries.len();
        if read > MAX_FEED_ENTRIES {
            return Ok(None);
        }
        for entry in page.entries {
            cursor = entry.seq.0;
            for scope in entry.scopes {
                match scope {
                    ChangeScope::Task { num, .. } => {
                        delta
                            .tasks
                            .insert(corral_core::Store::normalize_id(&num.0.to_string())?);
                    }
                    ChangeScope::Artifact { task } if task.starts_with("T-") => {
                        delta.tasks.insert(task);
                    }
                    ChangeScope::Lifecycle { task } if task.starts_with("T-") => {
                        delta.tasks.insert(task);
                    }
                    ChangeScope::Artifact { .. } | ChangeScope::Lifecycle { .. } => {
                        delta.assistants = true
                    }
                    ChangeScope::Record { area, .. } if area == "assistant-sessions" => {
                        delta.assistants = true
                    }
                    ChangeScope::Events { .. } => events_changed = true,
                    _ => {}
                }
            }
            if delta.first_change.is_none()
                && (!delta.tasks.is_empty() || delta.assistants || events_changed)
            {
                delta.first_change = Some(entry.at);
            }
            if delta.tasks.len() > MAX_TASKS {
                return Ok(None);
            }
        }
        if !page.more {
            break;
        }
        // A malformed/nonadvancing cache window must not make a query spin.
        if cursor <= previous || cursor >= after {
            return Ok(None);
        }
    }
    // Events are append-only in the canonical backend; reservation holes are harmless.
    if events_changed {
        delta.events_tail = view
            .events(
                &EventQuery {
                    newest_first: true,
                    ..EventQuery::default()
                },
                PageReq::first(1),
            )
            .context("reading search delta event boundary")?
            .items
            .first()
            .map_or(0, |row| row.seq.0);
    }
    if delta.events_tail < delta.events_after
        || delta.events_tail - delta.events_after > MAX_EVENT_DISTANCE
    {
        return Ok(None);
    }
    Ok(Some(delta))
}

pub(super) fn open(ctx: &Ctx) -> Result<Option<Catalog>> {
    let base = refresh::root(ctx)?;
    let Some(mut manifest) = refresh::read_manifest(&base)? else {
        return Ok(None);
    };
    let Some(delta) = plan(ctx, &manifest)? else {
        return Ok(None);
    };
    if !refresh::generation_available(&base, &manifest)? {
        return Ok(None);
    }
    let _path = corral_core::timing::span(if delta.has_changes() {
        "search.catalog.delta"
    } else {
        "search.catalog.generation"
    });
    let hints = base.join(&manifest.prefix_file);
    let temporary = tempfile::Builder::new()
        .prefix("corral-search-delta-")
        .tempdir()
        .context("creating query-local search delta")?;
    let root = temporary.path().to_path_buf();
    fs::create_dir(root.join("objects")).context("creating delta source cache")?;
    // Absolute partition paths retain the immutable generation; fresh partitions live only
    // in this query's temporary root. Source paths are rebased at each partition reader.
    for path in manifest.tasks.values_mut() {
        *path = base.join(&*path).to_string_lossy().into_owned();
    }
    for path in &mut manifest.events {
        *path = base.join(&*path).to_string_lossy().into_owned();
    }
    if let Some(path) = &mut manifest.assistants {
        *path = base.join(&*path).to_string_lossy().into_owned();
    }
    manifest.extras.clear();
    for id in &delta.tasks {
        let _span = corral_core::timing::span("search.catalog.delta_task");
        manifest.tasks.remove(id);
        let path = format!("task-{}.jsonl", super::digest(id));
        let mut writer = writer(&root, &path)?;
        let exists = fallback::task_records(ctx, id, |record| {
            write_record(&root, &base.join("objects"), record, &mut writer)
        })?;
        writer.flush().context("flushing canonical delta task")?;
        if exists {
            manifest.tasks.insert(id.clone(), path);
        }
    }
    if delta.assistants {
        let _span = corral_core::timing::span("search.catalog.delta_assistants");
        let path = "assistants.jsonl";
        let mut writer = writer(&root, path)?;
        fallback::assistant_records(ctx, |record| {
            write_record(&root, &base.join("objects"), record, &mut writer)
        })?;
        writer
            .flush()
            .context("flushing canonical delta assistants")?;
        manifest.assistants = Some(path.into());
    }
    if delta.events_tail > delta.events_after {
        fallback::write_events(
            ctx,
            &root,
            &mut manifest,
            delta.events_after,
            delta.events_tail,
            true,
        )?;
    }
    manifest.after = delta.after;
    // Re-run the SAME whole-corpus admission/projection as fallback, seeded from the SAME
    // global prefix cache, not the generation's old private statistics. This includes
    // displacement/admission of untouched documents and preserves exact IDF and task text.
    let cache = load_prefix_cache(ctx)?;
    let mut catalog = Catalog {
        objects: base.join("objects"),
        root,
        manifest,
        cache,
        temporary: Some(temporary),
    };
    catalog.rebuild_prefixes(ctx, false, true, Some(&hints))?;
    Ok(Some(catalog))
}

fn writer(root: &Path, name: &str) -> Result<BufWriter<File>> {
    Ok(BufWriter::new(
        File::create(root.join(name)).context("creating delta partition")?,
    ))
}

fn write_record(
    root: &Path,
    indexed: &Path,
    record: Record,
    writer: &mut impl Write,
) -> Result<()> {
    let _span = corral_core::timing::span("search.catalog.delta_source");
    let mut entry: Entry = metadata_entry(root, record)?;
    // A content-addressed body is unchanged even when its containing task/tier changed.
    // Missing lexical data is conservative: the shared scanner evaluates the source.
    if let Some(terms) = lexical(indexed, &entry.lexical)? {
        entry.terms = terms.terms;
        entry.fallback = terms.fallback;
    }
    serde_json::to_writer(&mut *writer, &entry).context("writing delta source metadata")?;
    writer
        .write_all(b"\n")
        .context("terminating delta source metadata")
}
