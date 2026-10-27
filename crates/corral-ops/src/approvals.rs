//! Approval attribution follows only `Events(approved)`. Rebuilding after cache loss or a
//! feed gap is a preparation step; callers can run [`prepare`] before opening a hot scope.
//! Rows contain event facts, not task titles/projects: those are point-read on the packet's
//! view, so renaming or moving a task never leaves stale attribution in a cached row.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::backend::{ChangeScope, ChangeSeq, EventQuery, FeedError, PageReq, StoredEvent};
use corral_core::{Event, Store};
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::integration::LandedTask;

#[path = "approvals_numbers.rs"]
pub(crate) mod numbers;

#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    #[serde(default)]
    version: u32,
    #[serde(with = "numbers::sequence")]
    change: ChangeSeq,
    #[serde(with = "numbers::number")]
    event: u64,
    #[serde(default, with = "numbers::number")]
    recomputations: u64,
}

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

fn row_path(root: &Path, sha: &str) -> PathBuf {
    root.join(format!(
        "{}.json",
        crate::verification::digest(sha.as_bytes())
    ))
}

/// Bring the approval rows to the caller's pinned view. Only new approved events are decoded
/// after the first call; resource/condition writes merely advance the feed cursor.
pub fn prepare(ctx: &Ctx) -> Result<()> {
    let _lock = lock(&ctx.store, "approval-rows")?;
    ctx.store
        .pass_derived("approval-rows-prepared-v2", || prepare_locked(ctx))
        .map(|_| ())
}

fn prepare_locked(ctx: &Ctx) -> Result<()> {
    let root = ctx.store.cache_root()?.join("approval-rows-v1");
    let path = root.join("cursor.json");
    let store = ctx.store.read_only_snapshot()?;
    let seq = store.view()?.seq();
    let previous: Option<Cursor> = read(&path)?;
    let delta = previous
        .as_ref()
        .filter(|c| c.version == 2 && c.change <= seq)
        .map(|c| changes(&store, c.change))
        .transpose()?
        .flatten();
    let rebuild = previous.is_none() || delta.is_none();
    let mut cursor = if rebuild {
        Cursor::default()
    } else {
        previous.unwrap_or_default()
    };
    if !rebuild && cursor.change == seq {
        return Ok(());
    }
    let relevant = rebuild
        || delta.as_ref().is_some_and(|scopes| {
            scopes.iter().any(
                |scope| matches!(scope, ChangeScope::Events { kind, .. } if kind == "approved"),
            )
        });
    if relevant {
        if rebuild && root.exists() {
            fs::remove_dir_all(&root).context("rebuilding approval rows")?;
        }
        let new_events = events(
            &store,
            &EventQuery {
                kinds: vec!["approved".into()],
                after: Some(corral_core::backend::EventSeq(cursor.event)),
                valid_only: true,
                ..EventQuery::default()
            },
        )?;
        let mut changed: BTreeMap<String, Vec<LandedTask>> = BTreeMap::new();
        for event in new_events {
            cursor.event = cursor.event.max(event.seq);
            if event.detail["strategy"].as_str() == Some("keep_branch") {
                continue;
            }
            let (Some(id), Some(sha)) = (event.task, event.detail["sha"].as_str()) else {
                continue;
            };
            let rows = match changed.entry(sha.to_owned()) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(read(&row_path(&root, sha))?.unwrap_or_default())
                }
            };
            let row = LandedTask {
                id,
                title: None,
                run: event.detail["run"].as_str().map(str::to_owned),
                approved_sha: sha.to_owned(),
            };
            if !rows.contains(&row) {
                rows.push(row);
            }
        }
        for (sha, rows) in changed {
            save(&row_path(&root, &sha), &rows)?;
            cursor.recomputations += 1;
        }
    }
    cursor.change = seq;
    cursor.version = 2;
    save(&path, &cursor)
}

#[cfg(test)]
pub(crate) fn recomputations(ctx: &Ctx) -> Result<u64> {
    let cursor: Option<Cursor> =
        read(&ctx.store.cache_root()?.join("approval-rows-v1/cursor.json"))?;
    Ok(cursor.map_or(0, |cursor| cursor.recomputations))
}

pub(crate) fn lookup(
    ctx: &Ctx,
    project: &Option<String>,
    shas: &BTreeSet<String>,
) -> Result<BTreeMap<String, Vec<LandedTask>>> {
    let _lock = lock(&ctx.store, "approval-rows")?;
    let root = ctx.store.cache_root()?.join("approval-rows-v1");
    let mut out = BTreeMap::new();
    for sha in shas {
        let mut rows: Vec<LandedTask> = read(&row_path(&root, sha))?.unwrap_or_default();
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
