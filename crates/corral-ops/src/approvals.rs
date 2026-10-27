//! Approval attribution comes from committed SHA-keyed facts on the evaluation's view.
//! Rows contain event facts, not task titles/projects: those are point-read on the packet's
//! view, so renaming or moving a task never leaves stale attribution in a cached row.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::path::Path;

use anyhow::{Context, Result};
use corral_core::backend::{ChangeScope, ChangeSeq, EventQuery, FeedError, PageReq, StoredEvent};
use corral_core::{Event, Store};
use serde::Serialize;

use crate::Ctx;
use crate::integration::LandedTask;

#[path = "approvals_numbers.rs"]
pub(crate) mod numbers;

/// Serialize derivation updates, including cache replacement. Store locks live outside
/// cache_root, so deletion of the rebuildable cache does not replace a held lock.
pub(crate) fn lock(store: &Store, name: &str) -> Result<File> {
    use fs2::FileExt;
    let path = store.cache_root()?.with_extension(format!("{name}.lock"));
    fs::create_dir_all(path.parent().context("derived lock has no parent")?)
        .context("creating derived lock directory")?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .context("opening derived row lock")?;
    file.lock_exclusive().context("locking derived rows")?;
    Ok(file)
}

pub(crate) fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    use corral_core::work::{self, Counter, Subject};
    work::counted(Counter::FileStats, 1);
    match fs::read(path) {
        Ok(bytes) => {
            work::decoded(
                Counter::RecordDecodes,
                u64::try_from(bytes.len())?,
                Subject::Plain,
            );
            Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
                format!("decoding derived rows {}", path.display())
            })?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("opening {}", path.display())),
    }
}

pub(crate) fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    fs::create_dir_all(path.parent().context("derived row has no parent")?)
        .context("creating derived row directory")?;
    crate::artifact_storage::replace(path, &serde_json::to_vec(value)?)
}

/// A feed gap returns `None`; other read errors are never treated as cache misses.
pub(crate) fn changes(store: &Store, after: ChangeSeq) -> Result<Option<Vec<ChangeScope>>> {
    let view = store.view()?;
    let mut after = after;
    let mut scopes = Vec::new();
    loop {
        let page = match view.changes(after, 256) {
            Ok(page) => page,
            Err(FeedError::Gap { .. }) => return Ok(None),
            Err(error) => return Err(error).context("reading derivation change feed"),
        };
        for entry in page.entries {
            after = entry.seq;
            scopes.extend(entry.scopes);
        }
        if !page.more {
            return Ok(Some(scopes));
        }
    }
}

pub(crate) fn event(row: StoredEvent) -> Result<Option<Event>> {
    if row.malformed {
        return Ok(None);
    }
    Ok(Some(Event {
        seq: row.seq.0,
        ts: row.event.ts,
        task: row.event.task,
        kind: row.event.kind,
        actor: row
            .event
            .actor
            .map(|actor| serde_json::from_value(actor.into()))
            .transpose()?,
        via: row.event.via,
        detail: row.event.detail,
    }))
}

pub(crate) fn events(store: &Store, query: &EventQuery) -> Result<Vec<Event>> {
    let view = store.view()?;
    let mut page = PageReq::first(256);
    let mut out = Vec::new();
    loop {
        let rows = view.events(query, page)?;
        for row in rows.items {
            if let Some(event) = event(row)? {
                out.push(event);
            }
        }
        match rows.next {
            Some(next) => {
                page = PageReq {
                    after: Some(next),
                    limit: 256,
                }
            }
            None => return Ok(out),
        }
    }
}

/// Validate the evaluation view. Attribution is maintained when events are committed;
/// existing databases initialize that projection at backend open, before HotScope.
pub fn prepare(ctx: &Ctx) -> Result<()> {
    ctx.store
        .view()?
        .ensure_fresh()
        .context("checking approval view")
}

#[cfg(test)]
pub(crate) fn recomputations(ctx: &Ctx) -> Result<u64> {
    // The maintained projection changes only on approved events, not record writes.
    Ok(u64::try_from(
        ctx.store.events_of(None, &["approved"])?.len(),
    )?)
}

pub(crate) fn lookup(
    ctx: &Ctx,
    project: &Option<String>,
    shas: &BTreeSet<String>,
) -> Result<BTreeMap<String, Vec<LandedTask>>> {
    let view = ctx.store.view()?;
    let mut out = BTreeMap::new();
    for sha in shas {
        let facts = view.lesson_approvals(sha)?;
        corral_core::work::decoded(
            corral_core::work::Counter::RecordDecodes,
            u64::try_from(serde_json::to_vec(&facts)?.len())?,
            corral_core::work::Subject::Plain,
        );
        let mut rows = facts
            .into_iter()
            .map(|fact| LandedTask {
                id: fact.task,
                title: None,
                run: fact.run,
                approved_sha: sha.clone(),
            })
            .collect::<Vec<_>>();
        let ids = rows
            .iter()
            .map(|row| row.id.clone())
            .collect::<BTreeSet<_>>();
        let heads = ctx.store.heads(&corral_core::backend::HeadQuery::Ids(
            ids.into_iter().collect(),
        ))?;
        rows.retain_mut(|row| {
            let task = heads.iter().find(|task| task.id == row.id);
            row.title = task.map(|task| task.title.clone());
            task.is_none_or(|task| task.project == *project)
        });
        rows.sort_by(|a, b| (&a.id, &a.run).cmp(&(&b.id, &b.run)));
        rows.dedup();
        out.insert(sha.clone(), rows);
    }
    Ok(out)
}
