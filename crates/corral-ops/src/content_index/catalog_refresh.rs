//! Follow only changed tasks and new events. Publication never acknowledges a newer view.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};

use anyhow::{Context, Result};
use corral_core::backend::{
    ChangeScope, ChangeSeq, EventQuery, EventSeq, FeedError, PageReq, TaskOrder, TaskPart,
    TaskQuery,
};

use super::{Catalog, Manifest, write_record};
use crate::Ctx;
use crate::content_index::{Record, Source};

const CATALOG_VERSION: u32 = 1;

pub(super) fn refresh(
    ctx: &Ctx,
    seq: ChangeSeq,
    catalog: &mut Catalog,
    reindex: bool,
) -> Result<bool> {
    let _span = corral_core::timing::span("search.index_refresh");
    let rebuild = reindex
        || catalog.manifest.version != CATALOG_VERSION
        || !ctx
            .store
            .cache_root()?
            .join("content-search.json")
            .is_file()
        || catalog.manifest.after > seq.0
        || catalog
            .manifest
            .tasks
            .iter()
            .any(|id| !catalog.root.join(format!("{id}.jsonl")).is_file())
        || (catalog.manifest.assistants && !catalog.root.join("assistants.jsonl").is_file())
        || std::fs::metadata(catalog.root.join(&catalog.manifest.events_file))
            .map(|metadata| metadata.len() < catalog.manifest.event_bytes)
            .unwrap_or(true);
    let delta = if rebuild {
        None
    } else {
        changes(ctx, ChangeSeq(catalog.manifest.after))?
    };
    let full = rebuild || delta.is_none();
    // The counter covers full source snapshots in either index implementation, not
    // incremental feed follows or prefix-only admission updates.
    #[cfg(test)]
    if full {
        crate::content_index::SNAPSHOTS.with(|count| count.set(count.get() + 1));
    }
    if !full && catalog.manifest.after == seq.0 {
        return Ok(false);
    }
    let (tasks, assistants, events) = match delta.filter(|_| !full) {
        Some(delta) => delta,
        None => {
            catalog.manifest = Manifest::default();
            let tasks = all_tasks(ctx)?;
            (tasks, true, true)
        }
    };
    for id in tasks {
        if ctx.store.head_if_exists(&id)?.is_some() {
            write_task(ctx, catalog, &id, reindex)?;
            catalog.manifest.tasks.insert(id);
        } else {
            catalog.manifest.tasks.remove(&id);
        }
    }
    if assistants {
        let mut file = tempfile::NamedTempFile::new_in(&catalog.root)
            .context("creating assistant search catalog")?;
        {
            let mut writer = BufWriter::new(file.as_file_mut());
            for session in ctx.store.list_assistants()? {
                for artifact in session.artifacts.iter().filter(|a| a.purged_at.is_none()) {
                    if crate::artifact_storage::content_available(ctx, artifact)? {
                        let record = Record::artifact(&session.id, artifact);
                        let cached = catalog.cache.documents.get(&record.document.reference);
                        write_record(ctx, &catalog.root, record, &mut writer, reindex, cached)?;
                    }
                }
            }
            writer
                .flush()
                .context("flushing assistant search catalog")?;
            writer
                .get_ref()
                .sync_all()
                .context("syncing assistant search catalog")?;
        }
        file.persist(catalog.root.join("assistants.jsonl"))
            .context("publishing assistant search catalog")?;
        catalog.manifest.assistants = true;
    }
    if events || full {
        let file = if full {
            let temp = tempfile::Builder::new()
                .prefix("events-")
                .suffix(".jsonl")
                .tempfile_in(&catalog.root)
                .context("creating event search partition")?;
            let (file, path) = temp.keep().context("retaining event search partition")?;
            catalog.manifest.events_file = path
                .file_name()
                .and_then(|name| name.to_str())
                .context("invalid event search partition name")?
                .to_string();
            file
        } else {
            OpenOptions::new()
                .write(true)
                .open(catalog.root.join(&catalog.manifest.events_file))
                .context("opening event candidate index")?
        };
        file.set_len(catalog.manifest.event_bytes)
            .context("discarding unpublished event search entries")?;
        let mut writer = BufWriter::new(file);
        use std::io::{Seek, SeekFrom};
        writer
            .seek(SeekFrom::End(0))
            .context("positioning event search index")?;
        let view = ctx.store.view()?;
        loop {
            let query = EventQuery {
                after: Some(EventSeq(catalog.manifest.events_after)),
                ..EventQuery::default()
            };
            let page = view
                .events(&query, PageReq::first(1))
                .context("reading unindexed search event")?;
            let Some(row) = page.items.into_iter().next() else {
                break;
            };
            catalog.manifest.events_after = row.seq.0;
            if let Some((document, detail)) = crate::content_index::events::stored_document(row)? {
                let cached = catalog.cache.documents.get(&document.reference);
                write_record(
                    ctx,
                    &catalog.root,
                    Record {
                        document,
                        source: Source::Text(detail),
                    },
                    &mut writer,
                    reindex,
                    cached,
                )?;
            }
        }
        writer.flush().context("flushing event search index")?;
        writer
            .get_ref()
            .sync_all()
            .context("syncing event search index")?;
        catalog.manifest.event_bytes = writer
            .stream_position()
            .context("reading event search index position")?;
    }
    catalog.manifest.version = CATALOG_VERSION;
    catalog.manifest.after = seq.0;
    Ok(true)
}

fn write_task(ctx: &Ctx, catalog: &Catalog, id: &str, reindex: bool) -> Result<()> {
    let mut records = Vec::new();
    // Reuse the row-based derivation used by the existing feed follower: no full Task,
    // receipts, checkpoint payloads, transcript history or unrelated row families.
    let changed = crate::content_index::follow::TaskDelta {
        parts: BTreeSet::from([
            TaskPart::Comment,
            TaskPart::Run,
            TaskPart::ReviewRun,
            TaskPart::Plan,
            TaskPart::PlanReview,
            TaskPart::Artifact,
        ]),
        history: true,
        demoted: false,
    };
    crate::content_index::follow::task(
        ctx,
        id,
        &changed,
        &crate::content_index::Delta::default(),
        &mut BTreeMap::new(),
        &mut records,
    )?;
    let mut file =
        tempfile::NamedTempFile::new_in(&catalog.root).context("creating task search catalog")?;
    {
        let mut writer = BufWriter::new(file.as_file_mut());
        for record in records {
            let cached = catalog.cache.documents.get(&record.document.reference);
            write_record(ctx, &catalog.root, record, &mut writer, reindex, cached)?;
        }
        writer.flush().context("flushing task search catalog")?;
        writer
            .get_ref()
            .sync_all()
            .context("syncing task search catalog")?;
    }
    file.persist(catalog.root.join(format!("{id}.jsonl")))
        .context("publishing task search catalog")?;
    Ok(())
}

fn all_tasks(ctx: &Ctx) -> Result<BTreeSet<String>> {
    let view = ctx.store.view()?;
    let query = TaskQuery {
        statuses: Vec::new(),
        project: None,
        order: TaskOrder::IdAsc,
    };
    let mut req = PageReq::first(256);
    let mut ids = BTreeSet::new();
    loop {
        let page = view
            .tasks(&query, req)
            .context("listing search task headers")?;
        ids.extend(page.items.into_iter().map(|header| header.id));
        let Some(after) = page.next else {
            break;
        };
        req = PageReq {
            after: Some(after),
            limit: 256,
        };
    }
    Ok(ids)
}

fn changes(ctx: &Ctx, mut after: ChangeSeq) -> Result<Option<(BTreeSet<String>, bool, bool)>> {
    let view = ctx.store.view()?;
    let (mut tasks, mut assistants, mut events) = (BTreeSet::new(), false, false);
    loop {
        let page = match view.changes(after, 256) {
            Ok(page) => page,
            Err(FeedError::Gap { .. }) => return Ok(None),
            Err(error) => return Err(error).context("following search catalog changes"),
        };
        for entry in page.entries {
            after = entry.seq;
            for scope in entry.scopes {
                match scope {
                    ChangeScope::Task { num, .. } => {
                        tasks.insert(corral_core::Store::normalize_id(&num.0.to_string())?);
                    }
                    ChangeScope::Artifact { task } if task.starts_with("T-") => {
                        tasks.insert(task);
                    }
                    ChangeScope::Artifact { .. } => assistants = true,
                    ChangeScope::Record { area, .. } if area == "assistant-sessions" => {
                        assistants = true
                    }
                    ChangeScope::Events { .. } => events = true,
                    _ => {}
                }
            }
        }
        if !page.more {
            break;
        }
    }
    Ok(Some((tasks, assistants, events)))
}
