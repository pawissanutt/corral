//! Explicit retention; dry runs simulate inode ownership without touching the store. A purge
//! also drops the task's blob reference, so blob GC frees the bytes once the grace horizon
//! passes; transcript retention (`[storage] transcript_retention_days`) purges transcripts.
use std::collections::{HashMap, HashSet};
use std::fs;

use anyhow::{Context, Result};
use corral_core::backend::BlobState;
use corral_core::backend::{Clock, SystemClock};
use corral_core::blob::GcRun;
use corral_core::blob_ref::BlobHash;
use corral_core::{
    Artifact, ArtifactOwner, ArtifactRetention, ArtifactTier, RetentionAction, Status,
};
use jiff::Timestamp;
use serde::Serialize;

use crate::Ctx;
use crate::artifact_storage::{
    ARTIFACT_BLOBS, blob_key, decoded_root, gzip, link_replace, protected, replace, storage_lock,
};

#[derive(Debug, Clone, Serialize)]
pub struct GcAction {
    pub task_id: String,
    pub path: String,
    /// "dedup", "compress", "purge", or "transcript" (a purge by transcript retention).
    pub action: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GcReport {
    pub dry_run: bool,
    pub actions: Vec<GcAction>,
    /// Reduction in unique stored content bytes, accounting for shared hardlinks.
    pub bytes_saved: u64,
    pub stored_before: u64,
    pub stored_after: u64,
    /// Blobs this run's blob GC pass deleted: only unreferenced ones past the grace horizon.
    pub blobs_deleted: u64,
    pub blob_bytes_freed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskStorageStats {
    pub task_id: String,
    pub original_bytes: u64,
    /// Unique bytes within this task; shared bytes can also belong to another task.
    pub stored_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageStats {
    pub original_bytes: u64,
    /// Unique bytes across the workspace, not the sum of task totals.
    pub stored_bytes: u64,
    pub tasks: Vec<TaskStorageStats>,
    pub warnings: Vec<String>,
}

#[derive(Clone)]
struct Entry {
    task_id: String,
    artifact: Artifact,
    identity: Option<String>,
    len: u64,
    transcript: bool,
}

fn identity(meta: &fs::Metadata, path: &str) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = path;
        format!("{}:{}", meta.dev(), meta.ino())
    }
    #[cfg(not(unix))]
    {
        // Paths are independent when the host cannot expose stable file identities.
        let _ = meta;
        path.to_string()
    }
}

fn entries(ctx: &Ctx, tasks: &[ArtifactOwner]) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for t in tasks {
        for a in &t.artifacts {
            let (identity, len) = if a.purged_at.is_some() {
                (None, 0)
            } else {
                match fs::metadata(ctx.store.dir().join(&a.path)) {
                    Ok(meta) => (Some(identity(&meta, &a.path)), meta.len()),
                    // Without its legacy copy an artifact is its blob.
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        let row = BlobHash::parse(&a.sha256)
                            .ok()
                            .map(|hash| ctx.store.blob_row(&hash))
                            .transpose()?
                            .flatten()
                            .filter(|row| row.state == BlobState::Live)
                            .with_context(|| {
                                format!("artifact {} has neither a snapshot nor a blob", a.path)
                            })?;
                        (Some(format!("{BLOB}{}", row.hash)), row.stored_bytes)
                    }
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("reading snapshot metadata {}", a.path));
                    }
                }
            };
            entries.push(Entry {
                task_id: t.id.clone(),
                transcript: !crate::transcripts::owners(ctx, &t.id, a)?.is_empty(),
                artifact: a.clone(),
                identity,
                len,
            });
        }
    }
    Ok(entries)
}

/// Identity prefix of an artifact stored only as its blob; legacy-copy dedup and compression
/// never touch it.
const BLOB: &str = "blob:";

fn blob_only(e: &Entry) -> bool {
    e.identity.as_deref().is_some_and(|id| id.starts_with(BLOB))
}

fn stored(entries: &[Entry]) -> u64 {
    let mut seen = HashSet::new();
    entries
        .iter()
        .filter(|e| e.identity.as_ref().is_some_and(|id| seen.insert(id)))
        .map(|e| e.len)
        .sum()
}

/// Reports original logical bytes and physically shared stored content bytes.
pub fn storage_stats(ctx: &Ctx) -> Result<StorageStats> {
    let tasks = ctx.store.artifact_owners()?;
    let entries = entries(ctx, &tasks)?;
    let rows: Vec<_> = tasks
        .iter()
        .map(|task| {
            let entries: Vec<_> = entries
                .iter()
                .filter(|e| e.task_id == task.id)
                .cloned()
                .collect();
            TaskStorageStats {
                task_id: task.id.clone(),
                original_bytes: entries.iter().map(|e| e.artifact.bytes).sum(),
                stored_bytes: stored(&entries),
            }
        })
        .collect();
    let mut warnings = Vec::new();
    let max_total = corral_core::parse_bytes(&ctx.store.config.artifacts.max_total)?;
    let max_file = corral_core::parse_bytes(&ctx.store.config.artifacts.max_file)?;
    let total = stored(&entries);
    if total > max_total {
        warnings.push(format!(
            "workspace artifacts store {total} bytes, exceeding max_total {max_total}"
        ));
    }
    for e in &entries {
        if e.identity.is_some() && e.len > max_file {
            warnings.push(format!(
                "{} artifact {} stores {} bytes, exceeding max_file {max_file}",
                e.task_id, e.artifact.path, e.len
            ));
        }
    }
    Ok(StorageStats {
        original_bytes: rows.iter().map(|r| r.original_bytes).sum(),
        stored_bytes: stored(&entries),
        tasks: rows,
        warnings,
    })
}

fn eligible(
    a: &Artifact,
    rule: &ArtifactRetention,
    finished: Option<Timestamp>,
    now: Timestamp,
) -> Result<bool> {
    if !rule.tiers.contains(&a.tier()) {
        return Ok(false);
    }
    let age = corral_core::parse_age(&rule.older_than)?;
    let old = finished.is_some_and(|at| now.duration_since(at) > age);
    let large = rule
        .larger_than
        .as_deref()
        .map(corral_core::parse_bytes)
        .transpose()?
        .is_some_and(|size| a.bytes > size);
    Ok(old || large)
}

enum Operation {
    Link {
        index: usize,
        source: usize,
        compressed_bytes: Option<u64>,
    },
    Compress {
        index: usize,
        bytes: Vec<u8>,
        source: Option<usize>,
    },
    Purge {
        index: usize,
    },
}

/// Applies workspace-wide dedup before retention. Protected snapshots are never replaced or removed.
pub fn artifact_gc(ctx: &Ctx, dry_run: bool) -> Result<GcReport> {
    gc(ctx, dry_run, false)
}

pub(crate) fn transcript_gc(ctx: &Ctx, dry_run: bool) -> Result<GcReport> {
    gc(ctx, dry_run, true)
}

fn gc(ctx: &Ctx, dry_run: bool, transcripts_only: bool) -> Result<GcReport> {
    // A dry run must not even create a lock/state file.
    let _lock = if dry_run {
        None
    } else {
        Some(storage_lock(ctx)?)
    };
    crate::assistant_artifacts::check_owner(None)?;
    let tasks = ctx.store.artifact_owners()?;
    let actual_tasks: Vec<_> = tasks
        .iter()
        .filter_map(|owner| owner.task().cloned())
        .collect();
    let protected = protected(ctx, &actual_tasks)?;
    let finishes = crate::finishes(&ctx.store, &actual_tasks)?;
    let sessions = ctx.store.assistant_artifact_owners()?;
    let mut entries = entries(ctx, &tasks)?;
    let before = stored(&entries);
    let mut actions = Vec::new();
    let mut operations = Vec::new();
    let mut canonical: HashMap<(String, bool), usize> = HashMap::new();
    // Protected copies are preferred as canonical sources, but never destinations.
    let mut order: Vec<_> = (0..entries.len()).collect();
    order.sort_by_key(|&i| !protected.contains(&entries[i].artifact.path));
    for i in order.into_iter().filter(|_| !transcripts_only) {
        let e = &entries[i];
        if e.identity.is_none() {
            continue;
        }
        let key = (
            e.artifact.sha256.clone(),
            e.artifact.compressed_bytes.is_some(),
        );
        if let Some(&source) = canonical.get(&key) {
            if !protected.contains(&e.artifact.path)
                && e.identity != entries[source].identity
                && !blob_only(e)
                && !blob_only(&entries[source])
            {
                operations.push(Operation::Link {
                    index: i,
                    source,
                    compressed_bytes: e.artifact.compressed_bytes.map(|_| entries[source].len),
                });
                entries[i].identity = entries[source].identity.clone();
                entries[i].len = entries[source].len;
                actions.push(action(&entries[i], "dedup"));
            }
        } else {
            canonical.insert(key, i);
        }
    }
    let clock = SystemClock;
    let now = clock.now();
    // Encoding keys refer only to compressed snapshots that survive this policy.
    let mut compressed: HashMap<String, usize> = HashMap::new();
    for (i, entry) in entries.iter().enumerate() {
        if entry.artifact.compressed_bytes.is_some() && entry.identity.is_some() {
            compressed.entry(entry.artifact.sha256.clone()).or_insert(i);
        }
    }
    for i in 0..entries.len() {
        let e = &entries[i];
        if transcripts_only || e.identity.is_none() || protected.contains(&e.artifact.path) {
            continue;
        }
        let task = tasks
            .iter()
            .find(|t| t.id == e.task_id)
            .context("artifact task missing")?;
        let rule = task
            .project
            .as_ref()
            .and_then(|p| ctx.store.config.projects.get(p))
            .and_then(|p| p.artifact_retention.as_ref())
            .unwrap_or(&ctx.store.config.artifacts.retention)
            .for_tier(e.artifact.tier());
        if !eligible(
            &e.artifact,
            &rule,
            finishes.get(&task.id).map(|f| f.at).or_else(|| {
                sessions
                    .iter()
                    .find(|s| s.id == task.id)
                    .filter(|s| s.status == corral_core::Status::Done)
                    .map(|s| s.updated)
            }),
            now,
        )? {
            continue;
        }
        match rule.action {
            RetentionAction::Keep => {}
            // Only transcript retention may purge a transcript (owner decision, plan section 10).
            RetentionAction::Purge if e.transcript => {}
            RetentionAction::Purge => {
                entries[i].identity = None;
                entries[i].len = 0;
                compressed.retain(|_, source| *source != i);
                operations.push(Operation::Purge { index: i });
                actions.push(action(&entries[i], "purge"));
            }
            RetentionAction::Compress => {
                // A blob is already compressed; only a legacy copy can be.
                if e.artifact.compressed_bytes.is_some() || blob_only(e) {
                    continue;
                }
                let bytes = gzip(&crate::artifact_read(ctx, &e.artifact)?)?;
                let source = compressed.get(&e.artifact.sha256).copied();
                let len = match source {
                    Some(source) => entries[source].len,
                    None => u64::try_from(bytes.len()).context("compressed length overflow")?,
                };
                if len >= e.len {
                    continue;
                }
                entries[i].identity = source
                    .and_then(|s| entries[s].identity.clone())
                    .or_else(|| Some(format!("gzip:{}", entries[i].artifact.sha256)));
                entries[i].len = len;
                operations.push(Operation::Compress {
                    index: i,
                    bytes,
                    source,
                });
                compressed
                    .entry(entries[i].artifact.sha256.clone())
                    .or_insert(i);
                actions.push(action(&entries[i], "compress"));
            }
        }
    }
    if let Some(retention) = ctx.store.config.storage.transcript_retention() {
        let retention = jiff::SignedDuration::try_from(retention)?;
        let kept: HashSet<&str> = tasks
            .iter()
            .flat_map(|t| &t.artifacts)
            .filter(|a| a.purged_at.is_none() && a.tier() == ArtifactTier::Keep)
            .map(|a| a.sha256.as_str())
            .collect();
        for (i, entry) in entries.iter_mut().enumerate() {
            let finished = tasks
                .iter()
                .find(|t| t.id == entry.task_id)
                .filter(|t| matches!(t.status, Status::Done | Status::Dropped))
                .and_then(|t| finishes.get(&t.id));
            if entry.identity.is_none()
                || !entry.transcript
                || protected.contains(&entry.artifact.path)
                || kept.contains(entry.artifact.sha256.as_str())
                || finished.is_none_or(|f| now.duration_since(f.at) <= retention)
            {
                continue;
            }
            entry.identity = None;
            entry.len = 0;
            operations.push(Operation::Purge { index: i });
            actions.push(action(entry, "transcript"));
        }
    }
    let after = stored(&entries);
    let bytes_saved = before.saturating_sub(after);
    let (mut blobs_deleted, mut blob_bytes_freed) = (0, 0);
    if !dry_run {
        for operation in operations {
            apply(ctx, &entries, operation, now)?;
        }
        let cache = decoded_root(ctx)?;
        if cache.exists() {
            fs::remove_dir_all(&cache).context("invalidating decoded artifact cache")?;
        }
        crate::content_index::snapshot(ctx, &ctx.store.list()?, false)?;
        if !transcripts_only && let GcRun::Done(report) = ctx.store.blob_gc(&clock)? {
            blobs_deleted = u64::try_from(report.deleted.len() + report.completed.len())?;
            blob_bytes_freed = report.freed_bytes;
        }
    }
    Ok(GcReport {
        dry_run,
        actions,
        bytes_saved,
        stored_before: before,
        stored_after: after,
        blobs_deleted,
        blob_bytes_freed,
    })
}

fn action(e: &Entry, kind: &str) -> GcAction {
    GcAction {
        task_id: e.task_id.clone(),
        path: e.artifact.path.clone(),
        action: kind.into(),
    }
}

fn apply(ctx: &Ctx, entries: &[Entry], operation: Operation, now: Timestamp) -> Result<()> {
    let (index, compressed, purged) = match operation {
        Operation::Link {
            index,
            source,
            compressed_bytes,
        } => {
            link_replace(
                &ctx.store.dir().join(&entries[source].artifact.path),
                &ctx.store.dir().join(&entries[index].artifact.path),
            )?;
            match compressed_bytes {
                Some(bytes) => (index, Some(bytes), None),
                None => return Ok(()),
            }
        }
        Operation::Compress {
            index,
            bytes,
            source,
        } => {
            let dest = ctx.store.dir().join(&entries[index].artifact.path);
            match source {
                Some(source) => {
                    link_replace(&ctx.store.dir().join(&entries[source].artifact.path), &dest)?
                }
                None => replace(&dest, &bytes)?,
            }
            (index, Some(entries[index].len), None)
        }
        Operation::Purge { index } => {
            match fs::remove_file(ctx.store.dir().join(&entries[index].artifact.path)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("purging artifact contents"),
            }
            (index, None, Some(now))
        }
    };
    let e = &entries[index];
    if e.task_id.starts_with("A-") {
        ctx.store.records_transaction(|| {
            crate::assistant_artifacts::update(ctx, &e.task_id, &e.artifact.path, |artifact| {
                artifact.compressed_bytes = compressed;
                artifact.purged_at = purged;
            })?;
            if purged.is_some()
                && !ctx
                    .store
                    .get_assistant(&e.task_id)?
                    .artifacts
                    .iter()
                    .any(|artifact| {
                        artifact.sha256 == e.artifact.sha256 && artifact.purged_at.is_none()
                    })
            {
                ctx.store
                    .delete_record(ARTIFACT_BLOBS, &blob_key(&e.task_id, &e.artifact.sha256))?;
            }
            Ok(())
        })?;
        return Ok(());
    }
    ctx.store.update(&e.task_id, |task| {
        let index = task
            .artifacts
            .iter()
            .position(|a| a.path == e.artifact.path)
            .context("artifact disappeared during retention")?;
        let a = &mut task.artifacts[index];
        a.compressed_bytes = compressed;
        a.purged_at = purged;
        let sha = a.sha256.clone();
        crate::artifact_storage::record_keys(ctx, task, index, false)?;
        // The task's blob reference goes with its last live artifact of that content.
        if purged.is_some()
            && !task
                .artifacts
                .iter()
                .any(|a| a.sha256 == sha && a.purged_at.is_none())
        {
            ctx.store
                .delete_record(ARTIFACT_BLOBS, &blob_key(&task.id, &sha))?;
        }
        Ok(())
    })?;
    Ok(())
}
