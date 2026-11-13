//! Query-local metadata catalog while the persistent complete-source index catches up.
//!
//! Task derivation holds only one short read view and one task's records at a time. Event
//! details are read in bounded pages and spooled to a single disposable body pack: neither
//! source extraction nor the eventual query pins a view over the accumulated history.
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use corral_core::backend::{EventQuery, EventSeq, PageReq, TaskOrder, TaskQuery};

use super::{Catalog, Entry, IndexedSource, Manifest, load_prefix_cache, metadata, write_metadata};
use crate::Ctx;
use crate::content_index::{Cache, Record};

const PAGE_SIZE: u32 = 64;

pub(super) fn build(ctx: &Ctx, reindex: bool) -> Result<Catalog> {
    let _span = corral_core::timing::span("search.fallback.prepare");
    #[cfg(test)]
    if reindex {
        crate::content_index::SNAPSHOTS.with(|count| count.set(count.get() + 1));
    }
    let temporary = tempfile::Builder::new()
        .prefix("corral-search-fallback-")
        .tempdir()
        .context("creating temporary search catalog")?;
    let root = temporary.path().to_path_buf();
    let objects = root.join("objects");
    fs::create_dir(&objects).context("creating temporary search source directory")?;
    let (after, event_tail) = ctx.read_step(|read| {
        let view = read.store.view()?;
        let page = view
            .events(
                &EventQuery {
                    newest_first: true,
                    ..EventQuery::default()
                },
                PageReq::first(1),
            )
            .context("reading fallback search event boundary")?;
        Ok((view.seq().0, page.items.first().map_or(0, |row| row.seq.0)))
    })?;
    let mut manifest = Manifest {
        version: 1,
        after,
        ..Manifest::default()
    };
    write_tasks(ctx, &root, &mut manifest)?;
    write_assistants(ctx, &root, &mut manifest)?;
    write_events(ctx, &root, &mut manifest, event_tail)?;
    let cache = if reindex {
        Cache::default()
    } else {
        load_prefix_cache(ctx)?
    };
    let mut catalog = Catalog {
        root,
        objects,
        manifest,
        cache,
        temporary: Some(temporary),
    };
    // Only admitted prefixes are read. All other bodies remain untouched until the query
    // streams its eligible records; no full-source terms or grams are computed here.
    catalog.rebuild_prefixes(ctx, reindex, true)?;
    Ok(catalog)
}

fn writer(root: &Path, name: &str) -> Result<BufWriter<File>> {
    Ok(BufWriter::new(
        File::create(root.join(name)).context("creating temporary search partition")?,
    ))
}

fn write_tasks(ctx: &Ctx, root: &Path, manifest: &mut Manifest) -> Result<()> {
    let query = TaskQuery {
        statuses: Vec::new(),
        project: None,
        order: TaskOrder::IdAsc,
    };
    let mut request = PageReq::first(PAGE_SIZE);
    loop {
        let page = ctx.read_step(|read| {
            read.store
                .view()?
                .tasks(&query, request.clone())
                .context("listing fallback search task headers")
        })?;
        for header in page.items {
            let Some((mut scan, spec)) = metadata::begin(ctx, &header.id)? else {
                continue;
            };
            let name = format!("task-{}.jsonl", manifest.tasks.len());
            let mut partition = writer(root, &name)?;
            write_metadata(root, spec, &mut partition)?;
            loop {
                // The scanner releases its short view before returning each bounded page.
                let (records, done) = metadata::next(ctx, &mut scan)?;
                for record in records {
                    write_metadata(root, record, &mut partition)?;
                }
                if done {
                    break;
                }
            }
            partition
                .flush()
                .context("flushing temporary task search partition")?;
            manifest.tasks.insert(header.id, name);
        }
        let Some(after) = page.next else {
            break;
        };
        request.after = Some(after);
    }
    Ok(())
}

fn write_assistants(ctx: &Ctx, root: &Path, manifest: &mut Manifest) -> Result<()> {
    let sessions = ctx.read_step(|read| read.store.list_assistants())?;
    let name = "assistants.jsonl";
    let mut partition = writer(root, name)?;
    for session in sessions {
        for artifact in session.artifacts.iter().filter(|a| a.purged_at.is_none()) {
            if crate::artifact_storage::content_available(ctx, artifact)? {
                write_metadata(
                    root,
                    Record::artifact(&session.id, artifact),
                    &mut partition,
                )?;
            }
        }
    }
    partition
        .flush()
        .context("flushing temporary assistant search partition")?;
    manifest.assistants = Some(name.into());
    Ok(())
}

fn write_events(ctx: &Ctx, root: &Path, manifest: &mut Manifest, tail: u64) -> Result<()> {
    let name = "events.jsonl";
    let body_name = "events.body";
    let mut partition = writer(root, name)?;
    let mut bodies = writer(root, body_name)?;
    let mut offset = 0_u64;
    let mut after = 0_u64;
    'events: while after < tail {
        let query = EventQuery {
            after: Some(EventSeq(after)),
            ..EventQuery::default()
        };
        let page = ctx.read_step(|read| {
            read.store
                .view()?
                .events(&query, PageReq::first(PAGE_SIZE))
                .context("reading fallback search event page")
        })?;
        if page.items.is_empty() {
            break;
        }
        for row in page.items {
            if row.seq.0 > tail {
                break 'events;
            }
            after = row.seq.0;
            let Some((document, detail)) = crate::content_index::events::stored_document(row)?
            else {
                continue;
            };
            let len = u64::try_from(detail.len()).context("event search body size overflow")?;
            bodies
                .write_all(detail.as_bytes())
                .context("writing temporary event search body")?;
            let entry = Entry {
                lexical: format!("e-{after}"),
                document,
                source: IndexedSource::Slice {
                    file: body_name.into(),
                    offset,
                    len,
                },
                terms: None,
                fallback: None,
            };
            offset = offset
                .checked_add(len)
                .context("event search body offset overflow")?;
            serde_json::to_writer(&mut partition, &entry)
                .context("writing temporary event search entry")?;
            partition
                .write_all(b"\n")
                .context("terminating temporary event search entry")?;
        }
    }
    bodies
        .flush()
        .context("flushing temporary event search bodies")?;
    partition
        .flush()
        .context("flushing temporary event search partition")?;
    manifest.events_after = after;
    manifest.events.push(name.into());
    Ok(())
}
