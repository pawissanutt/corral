//! The storage classes `storage gc` reports, one collector each. A collector in a dry run reads
//! only; with `--yes` it removes its own class's reclaimable entries and nothing else.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use corral_core::backend::{BlobRow, BlobState, ScratchState, SystemClock};
use corral_core::blob::GcRun;
use corral_core::scratch::{ClassDeleter, Registration};
use corral_core::{ArtifactKind, Status, StorageClass, Store};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use super::worktrees::{self, Finding};
use super::{Bucket, Disposition, GcReport, entry, lifecycle};
use crate::Ctx;
use crate::scratch_reclaim::usage;

fn size(path: &Path) -> Result<u64> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => usage(path),
        Ok(meta) => Ok(meta.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error).with_context(|| format!("inspecting {}", path.display())),
    }
}

fn listing(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("listing {}", dir.display())),
    };
    let mut paths = Vec::new();
    for entry in entries {
        paths.push(
            entry
                .with_context(|| format!("listing {}", dir.display()))?
                .path(),
        );
    }
    paths.sort();
    Ok(paths)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Every directory below each project's `.worktrees`: finished worktrees by the lifecycle
/// policy, the rest reported with the reason they stay.
pub(super) fn worktrees(ctx: &Ctx, now: Timestamp, report: &mut GcReport) -> Result<()> {
    let digest = lifecycle::policy_digest(ctx)?;
    for project in ctx.store.config.projects.keys() {
        let repo = ctx.store.project_path(project)?;
        for path in listing(&repo.join(".worktrees"))? {
            let bytes = size(&path)?;
            let owner = Store::normalize_id(&file_name(&path))
                .ok()
                .and_then(|id| ctx.store.head(&id).ok())
                .filter(|head| {
                    head.current_run
                        .as_deref()
                        .is_some_and(|run| run.worktree == path)
                });
            let Some(head) = owner else {
                report.push(entry(
                    "worktree",
                    path.display().to_string(),
                    bytes,
                    Disposition::Retained,
                    Some("unmanaged: no corral task's last run owns it".into()),
                ));
                continue;
            };
            let Some(found) = worktrees::assess(ctx, &head)? else {
                continue;
            };
            let finished = matches!(head.status, Status::Done | Status::Dropped);
            let mut line = match &found.finding {
                Finding::Reclaim { .. } => {
                    report.removable("worktree", path.display().to_string(), bytes)
                }
                Finding::Retain(reason) => entry(
                    "worktree",
                    path.display().to_string(),
                    bytes,
                    Disposition::Retained,
                    Some(reason.clone()),
                ),
                Finding::Defer(reason) => entry(
                    "worktree",
                    path.display().to_string(),
                    bytes,
                    Disposition::Deferred,
                    Some(reason.clone()),
                ),
            };
            line.task = Some(head.id.clone());
            if !report.dry_run && finished {
                let stored =
                    ctx.store
                        .lifecycle_row(&head.id, &found.generation, worktrees::TARGET)?;
                let (outcome, reason) =
                    lifecycle::record(ctx, &found, stored.as_ref(), &digest, now, "gc")?;
                if outcome == corral_core::backend::LifecycleOutcome::Failed {
                    line.disposition = Disposition::Failed;
                    line.reason = reason;
                }
            }
            report.push(line);
        }
    }
    Ok(())
}

/// Registered scratch directories: open ones stay; closed and deferred ones go when nothing
/// uses them.
pub(super) fn scratch(ctx: &Ctx, now: Timestamp, report: &mut GcReport) -> Result<()> {
    let states = [
        ScratchState::Open,
        ScratchState::Closed,
        ScratchState::Deferred,
        ScratchState::Reclaiming,
        ScratchState::Failed,
    ];
    let rows = ctx.store.scratch_rows_viewed(&states)?;
    let lease = match report.dry_run || rows.is_empty() {
        true => None,
        false => Some(ctx.store.try_lock(crate::scratch_reclaim::LEASE)?),
    };
    for row in rows {
        let dir = ctx.store.scratch_path(&row.path)?;
        let bytes = size(&dir)?;
        let target = dir.display().to_string();
        let mut line = match row.state {
            ScratchState::Open => entry(
                "scratch",
                target,
                bytes,
                Disposition::Retained,
                Some("attempt open".into()),
            ),
            ScratchState::Reclaiming => entry(
                "scratch",
                target,
                bytes,
                Disposition::Retained,
                Some("reclaim in progress; sync completes it".into()),
            ),
            ScratchState::Failed => entry(
                "scratch",
                target,
                bytes,
                Disposition::Retained,
                Some(format!(
                    "reclaim failed: {}",
                    row.reason.as_deref().unwrap_or("unknown")
                )),
            ),
            _ if report.dry_run => match crate::scratch_reclaim::in_use_now(ctx, &row, now)? {
                Some(reason) => entry(
                    "scratch",
                    target,
                    bytes,
                    Disposition::Deferred,
                    Some(reason),
                ),
                None => report.removable("scratch", target, bytes),
            },
            _ => match &lease {
                Some(Some(_)) => match crate::scratch_reclaim::reclaim(ctx, &row, now)? {
                    Some(reason) => entry(
                        "scratch",
                        target,
                        bytes,
                        Disposition::Deferred,
                        Some(reason),
                    ),
                    None => report.removable("scratch", target, bytes),
                },
                _ => entry(
                    "scratch",
                    target,
                    bytes,
                    Disposition::Retained,
                    Some("a sync pass is reclaiming scratch".into()),
                ),
            },
        };
        line.task = row.task.clone();
        report.push(line);
    }
    Ok(())
}

/// `.corral/agent/sessions/<task>`: a terminal task's sessions go once every transcript file
/// is promoted to an artifact (`--yes` promotes the rest first); open tasks, the PM's sessions
/// and unknown directories stay.
pub(super) fn agent_sessions(ctx: &Ctx, report: &mut GcReport) -> Result<()> {
    let layout = ctx.store.storage_layout()?;
    let root = layout.class_root(StorageClass::AgentSessions);
    for path in listing(&root)? {
        let name = file_name(&path);
        let bytes = size(&path)?;
        let target = path.display().to_string();
        let retain = |reason: &str| {
            entry(
                "agent-session",
                target.clone(),
                bytes,
                Disposition::Retained,
                Some(reason.into()),
            )
        };
        let head = Store::normalize_id(&name)
            .ok()
            .and_then(|id| ctx.store.head(&id).ok());
        let Some(head) = head.filter(|head| head.id == name) else {
            report.push(retain(if name == "pm" {
                "PM sessions"
            } else {
                "unknown: no such task"
            }));
            continue;
        };
        if !matches!(head.status, Status::Done | Status::Dropped) {
            let mut line = retain("non-terminal: task is open");
            line.task = Some(head.id.clone());
            report.push(line);
            continue;
        }
        let mut line = report.removable("agent-session", target.clone(), bytes);
        line.task = Some(head.id.clone());
        line.bucket = Bucket::CompressedIntoBlobs;
        match session_files(&path)? {
            None => {
                line.disposition = Disposition::Retained;
                line.reason = Some("holds a symlink or special file".into());
            }
            Some(files) => {
                let busy = match worktrees::in_use(&path)? {
                    Some(reason) => Some(reason),
                    None => pane_busy(ctx, &head)?,
                };
                if let Some(reason) = busy {
                    line.disposition = Disposition::Deferred;
                    line.reason = Some(reason);
                } else {
                    let owed = unpromoted(ctx, &head.id, &files)?;
                    line.reason = Some(format!(
                        "{} transcript files promoted, {} to promote",
                        files.len() - owed.len(),
                        owed.len()
                    ));
                    if !report.dry_run
                        && let Err(error) =
                            promote_and_remove(ctx, &layout, &head, &files, &owed, bytes, &path)
                    {
                        line.disposition = Disposition::Failed;
                        line.reason = Some(format!("{error:#}"));
                    }
                }
            }
        }
        report.push(line);
    }
    Ok(())
}

/// Regular files below a task's sessions directory, each with the run directory it sits in;
/// `None` when anything else (a symlink, a socket) is there.
fn session_files(dir: &Path) -> Result<Option<Vec<(String, PathBuf)>>> {
    let mut files = Vec::new();
    let mut pending = vec![(None::<String>, dir.to_path_buf())];
    while let Some((run, next)) = pending.pop() {
        for path in listing(&next)? {
            let kind = std::fs::symlink_metadata(&path)?.file_type();
            if kind.is_dir() {
                pending.push((run.clone().or_else(|| Some(file_name(&path))), path));
            } else if kind.is_file() {
                files.push((run.clone().unwrap_or_default(), path));
            } else {
                return Ok(None);
            }
        }
    }
    files.sort();
    Ok(Some(files))
}

/// Why a terminal task's sessions are still in use: one of its unreleased panes is still
/// working, or its state cannot be read.
fn pane_busy(ctx: &Ctx, head: &corral_core::TaskHead) -> Result<Option<String>> {
    for pane in head.unreleased_panes(false)? {
        if pane.pane.is_empty() {
            continue;
        }
        match ctx.herdr.pane_get(&pane.pane) {
            Ok(Some(info)) if info.status == corral_herdr::AgentStatus::Working => {
                return Ok(Some(format!("pane {} is still working", pane.pane)));
            }
            Ok(_) => {}
            Err(error) => {
                return Ok(Some(format!(
                    "pane {} cannot be checked: {error:#}",
                    pane.pane
                )));
            }
        }
    }
    Ok(None)
}

/// The transcript files of `task` without a live (unpurged) artifact snapshot of their exact
/// current bytes.
fn unpromoted<'a>(
    ctx: &Ctx,
    task: &str,
    files: &'a [(String, PathBuf)],
) -> Result<Vec<&'a (String, PathBuf)>> {
    let live: std::collections::BTreeSet<String> = ctx
        .store
        .artifact_rows(task, None)?
        .into_iter()
        .filter(|artifact| artifact.purged_at.is_none())
        .map(|artifact| artifact.sha256)
        .collect();
    let mut owed = Vec::new();
    for entry in files {
        let bytes =
            std::fs::read(&entry.1).with_context(|| format!("reading {}", entry.1.display()))?;
        if !live.contains(&corral_core::agent::paths::sha256_hex(&bytes)) {
            owed.push(entry);
        }
    }
    Ok(owed)
}

fn promote_and_remove(
    ctx: &Ctx,
    layout: &corral_core::StorageLayout,
    head: &corral_core::TaskHead,
    files: &[(String, PathBuf)],
    owed: &[&(String, PathBuf)],
    bytes: u64,
    path: &Path,
) -> Result<()> {
    let current = head.current_run.as_deref().map(|run| run.id.as_str());
    for (run, file) in owed {
        let title = format!("agent session transcript {}", file.display());
        let kind = Some(ArtifactKind::Log);
        if Some(run.as_str()) == current {
            crate::add_artifact(ctx, &head.id, file, kind, Some(&title))?;
        } else {
            crate::artifacts::add_artifact_to_run(ctx, &head.id, run, file, kind, Some(&title))?;
        }
    }
    let deleter = ClassDeleter::open(layout, StorageClass::AgentSessions)?;
    let registration = Registration {
        class: StorageClass::AgentSessions,
        id: head.id.clone(),
        rel: PathBuf::from(&head.id),
    };
    // Immediately before deletion: still terminal, and every transcript file's exact current
    // bytes have a live snapshot (a purged or older snapshot of the same path does not count).
    let safe = || {
        let status = ctx.store.head(&head.id)?.status;
        anyhow::ensure!(
            matches!(status, Status::Done | Status::Dropped),
            "{} was reopened",
            head.id
        );
        let missing = unpromoted(ctx, &head.id, files)?;
        anyhow::ensure!(
            missing.is_empty(),
            "transcript {} has no live snapshot of its current bytes",
            missing[0].1.display()
        );
        Ok(())
    };
    deleter.remove(&registration, &safe)?;
    ctx.store.log_event(
        Some(&head.id),
        "storage_reclaimed",
        json!({ "class": "agent-session", "path": path, "bytes": bytes, "promoted": owed.len(), "via": "gc" }),
    )
}

/// Legacy paths awaiting `migrate --finalize`, reported, and the build output (directories
/// tagged with `CACHEDIR.TAG`) under the legacy state area, which `--yes` removes.
pub(super) fn legacy(ctx: &Ctx, report: &mut GcReport) -> Result<()> {
    const LEGACY: [&str; 6] = [
        "tasks",
        "state",
        "artifacts",
        "prompts",
        "events.jsonl",
        "lessons",
    ];
    let corral = ctx.store.dir();
    for name in LEGACY {
        let path = corral.join(name);
        if std::fs::symlink_metadata(&path).is_err() {
            continue;
        }
        let mut build = Vec::new();
        // A symlinked state area is never scanned, so nothing outside `.corral` is reached.
        let real_dir = std::fs::symlink_metadata(&path)?.is_dir();
        if name == "state" && real_dir {
            tagged_dirs(&path, &mut build)?;
        }
        let mut build_bytes = 0;
        for dir in build {
            let bytes = size(&dir)?;
            build_bytes += bytes;
            let target = dir.display().to_string();
            if let Some(reason) = worktrees::in_use(&dir)? {
                report.push(entry(
                    "legacy-build-output",
                    target,
                    bytes,
                    Disposition::Deferred,
                    Some(reason),
                ));
                continue;
            }
            let mut line = report.removable("legacy-build-output", target, bytes);
            if !report.dry_run {
                let rel = dir.strip_prefix(&path)?;
                let still_build_output = || {
                    anyhow::ensure!(
                        dir.join("CACHEDIR.TAG").is_file(),
                        "{} is no longer tagged",
                        dir.display()
                    );
                    match worktrees::in_use(&dir)? {
                        Some(reason) => anyhow::bail!("{reason}"),
                        None => Ok(()),
                    }
                };
                match worktrees::remove_below(&path, rel, &still_build_output) {
                    Ok(()) => ctx.store.log_event(
                        None,
                        "storage_reclaimed",
                        json!({ "class": "legacy-build-output", "path": dir, "bytes": bytes, "via": "gc" }),
                    )?,
                    Err(error) => {
                        line.disposition = Disposition::Failed;
                        line.reason = Some(format!("removing {}: {error:#}", dir.display()));
                    }
                }
            }
            report.push(line);
        }
        let mut line = entry(
            "legacy",
            path.display().to_string(),
            size(&path)?.saturating_sub(if report.dry_run { build_bytes } else { 0 }),
            Disposition::Reported,
            Some("legacy store files, removed by the migration's finalize step".into()),
        );
        line.bucket = Bucket::LegacyAwaitingFinalize;
        report.push(line);
    }
    Ok(())
}

/// Directories below `dir` holding `CACHEDIR.TAG`, without descending into them or following
/// symlinks.
fn tagged_dirs(dir: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    for path in listing(dir)? {
        if !std::fs::symlink_metadata(&path)?.is_dir() {
            continue;
        }
        if path.join("CACHEDIR.TAG").is_file() {
            found.push(path);
        } else {
            tagged_dirs(&path, found)?;
        }
    }
    Ok(())
}

/// Evidence blobs: the live footprint, then each unreferenced blob and stray file, removable
/// once past the horizon (blob GC under the `storage-gc` claim does the deleting).
pub(super) fn blobs(ctx: &Ctx, now: Timestamp, report: &mut GcReport) -> Result<()> {
    let rows = ctx.store.blob_rows()?;
    let (live, raw) = rows
        .iter()
        .filter(|row| row.state == BlobState::Live)
        .fold((0, 0), |(s, r), row| {
            (s + row.stored_bytes, r + row.raw_bytes)
        });
    let mut footprint = entry(
        "evidence-blobs",
        "live blobs",
        live,
        Disposition::Reported,
        Some(format!("{raw} raw bytes stored compressed")),
    );
    footprint.bucket = Bucket::CompressedIntoBlobs;
    report.push(footprint);
    if !report.dry_run {
        return collect_blobs(ctx, &rows, report);
    }
    let grace = SignedDuration::try_from(ctx.store.config.storage.blob_grace())?;
    let grace_start = now.checked_sub(grace)?;
    let horizon = ctx
        .store
        .storage_leases()?
        .into_iter()
        .filter(|lease| lease.expires > now)
        .map(|lease| lease.started)
        .fold(grace_start, Timestamp::min);
    let known: BTreeMap<String, &BlobRow> =
        rows.iter().map(|row| (row.hash.to_string(), row)).collect();
    for row in &rows {
        let unreferenced =
            row.refs == 0 && matches!(row.state, BlobState::Live | BlobState::Writing);
        let target = format!("blob:sha256:{}", row.hash.as_str());
        let collectable = unreferenced
            && row.reservations.iter().all(|r| r.expires <= now)
            && row.unreferenced_since.unwrap_or(row.created) < horizon;
        if row.state == BlobState::Deleting || collectable {
            report.push(report.removable("blob", target, row.stored_bytes));
        } else if unreferenced {
            report.push(entry(
                "blob",
                target,
                row.stored_bytes,
                Disposition::Retained,
                Some("unreferenced within the grace period or reserved".into()),
            ));
        }
    }
    for path in stray_files(&ctx.store.dir().join("blobs").join("sha256"))? {
        let name = format!(
            "{}{}",
            path.parent().map(file_name).unwrap_or_default(),
            file_name(&path)
        );
        if known.contains_key(&name) {
            continue;
        }
        let meta = std::fs::symlink_metadata(&path)?;
        let modified = Timestamp::try_from(meta.modified().unwrap_or(SystemTime::UNIX_EPOCH))?;
        let target = path.display().to_string();
        if modified < grace_start {
            report.push(report.removable("blob-stray", target, meta.len()));
        } else {
            report.push(entry(
                "blob-stray",
                target,
                meta.len(),
                Disposition::Retained,
                Some("stray within the grace period".into()),
            ));
        }
    }
    Ok(())
}

fn stray_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for fanout in listing(dir)? {
        if std::fs::symlink_metadata(&fanout)?.is_dir() {
            files.extend(listing(&fanout)?);
        }
    }
    Ok(files)
}

fn collect_blobs(ctx: &Ctx, rows: &[BlobRow], report: &mut GcReport) -> Result<()> {
    let blobs = ctx.store.blob_store()?;
    let mut bytes: BTreeMap<String, u64> = rows
        .iter()
        .map(|row| (row.hash.to_string(), row.stored_bytes))
        .collect();
    for path in stray_files(&ctx.store.dir().join("blobs").join("sha256"))? {
        let name = format!(
            "{}{}",
            path.parent().map(file_name).unwrap_or_default(),
            file_name(&path)
        );
        bytes.entry(name).or_insert(size(&path)?);
    }
    let gc = match ctx.store.blob_gc(&SystemClock)? {
        GcRun::Done(gc) => gc,
        GcRun::ClaimHeld { owner } => {
            report.push(entry(
                "blob",
                "storage-gc claim",
                0,
                Disposition::Retained,
                Some(format!("blob GC claim held by {owner}")),
            ));
            return Ok(());
        }
    };
    let removed = gc
        .completed
        .iter()
        .chain(&gc.deleted)
        .map(|hash| ("blob", hash));
    for (class, hash) in removed.chain(gc.strays.iter().map(|hash| ("blob-stray", hash))) {
        let size = bytes.get(&hash.to_string()).copied().unwrap_or(0);
        ctx.store.log_event(
            None,
            "storage_reclaimed",
            json!({ "class": class, "blob": hash, "bytes": size, "via": "gc" }),
        )?;
        let target = match class {
            "blob-stray" => blobs.path_of(hash).display().to_string(),
            _ => format!("blob:sha256:{}", hash.as_str()),
        };
        report.push(report.removable(class, target, size));
    }
    for kept in &gc.kept {
        report.push(entry(
            "blob",
            format!("blob:sha256:{}", kept.hash.as_str()),
            0,
            Disposition::Retained,
            Some(kept.reason.clone()),
        ));
    }
    Ok(())
}

/// Fixed class roots, rollback archives and the configurable roots outside `.corral`: counted,
/// never deleted.
pub(super) fn roots(ctx: &Ctx, report: &mut GcReport) -> Result<()> {
    let corral = ctx.store.dir();
    for name in [
        "db",
        "db.migrating",
        "blobs",
        "agent/sessions",
        "lock",
        "rollback",
    ] {
        let path = corral.join(name);
        if std::fs::symlink_metadata(&path).is_err() {
            continue;
        }
        let mut line = entry(
            "fixed-root",
            path.display().to_string(),
            size(&path)?,
            Disposition::Reported,
            Some("fixed class root, never deleted".into()),
        );
        line.bucket = Bucket::FixedRoot;
        report.push(line);
    }
    for archive in listing(&corral.join("rollback"))? {
        let mut line = entry(
            "rollback-archive",
            archive.display().to_string(),
            size(&archive)?,
            Disposition::Reported,
            Some("rollback archive, never deleted automatically".into()),
        );
        line.bucket = Bucket::RollbackArchive;
        report.push(line);
    }
    let layout = ctx.store.storage_layout()?;
    for (class, root) in [
        ("cache-root", layout.cache_root()),
        ("scratch-root", layout.scratch_root()),
    ] {
        let mut line = entry(
            class,
            root.display().to_string(),
            size(root)?,
            Disposition::Reported,
            Some("outside .corral".into()),
        );
        line.bucket = Bucket::MovedOutsideCorral;
        report.push(line);
    }
    Ok(())
}
