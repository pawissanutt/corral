//! Cheap observations of search inputs, without reading artifacts, events, or the index.
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result};
use corral_core::{Run, Task};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::Ctx;

#[cfg(test)]
mod tests;

fn hash_value(hash: &mut Sha256, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value).context("serializing search input fingerprint")?;
    hash.update(bytes);
    Ok(())
}

fn file_inputs(hash: &mut Sha256, path: &Path) -> Result<()> {
    if is_artifact_snapshot(path) {
        corral_core::work::record(corral_core::work::Work::ArtifactStat, 1);
    }
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            hash.update([0]);
            return Ok(());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("inspecting {}", path.display()));
        }
    };
    hash.update([1]);
    hash.update(metadata.len().to_le_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hash.update(metadata.dev().to_le_bytes());
        hash.update(metadata.ino().to_le_bytes());
        hash.update(metadata.mtime().to_le_bytes());
        hash.update(metadata.mtime_nsec().to_le_bytes());
        hash.update(metadata.ctime().to_le_bytes());
        hash.update(metadata.ctime_nsec().to_le_bytes());
    }
    #[cfg(not(unix))]
    {
        let modified = metadata
            .modified()
            .context("reading search input mtime")?
            .duration_since(std::time::UNIX_EPOCH)
            .context("search input mtime precedes epoch")?;
        hash.update(modified.as_secs().to_le_bytes());
        hash.update(modified.subsec_nanos().to_le_bytes());
    }
    Ok(())
}

/// Artifact snapshots live at `artifacts/<task>/<run>/<file>`; only they count as artifact stats.
fn is_artifact_snapshot(path: &Path) -> bool {
    path.ancestors()
        .nth(3)
        .and_then(Path::file_name)
        .is_some_and(|name| name == "artifacts")
}

pub(super) fn file_stamp(path: &Path) -> Result<String> {
    let mut hash = Sha256::new();
    file_inputs(&mut hash, path)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn run_inputs(hash: &mut Sha256, run: &Run) -> Result<()> {
    hash_value(
        hash,
        &(
            &run.id,
            &run.summary,
            run.review
                .as_ref()
                .map(|review| (review.verdict, &review.notes, review.at)),
        ),
    )?;
    hash_value(hash, &run.review_runs.len())?;
    for child in &run.review_runs {
        run_inputs(hash, child)?;
    }
    Ok(())
}

fn task_inputs(tasks: &[Task]) -> Result<([u8; 32], u64)> {
    use corral_core::work::{Work, count};
    let stats = count(Work::ArtifactStat);
    let mut hash = Sha256::new();
    hash_value(&mut hash, &tasks.len())?;
    for task in tasks {
        hash_value(
            &mut hash,
            &(&task.id, &task.title, task.updated, &task.body),
        )?;
        hash_value(&mut hash, &task.comments.len())?;
        for comment in &task.comments {
            hash_value(&mut hash, &comment.text)?;
        }
        hash_value(&mut hash, &task.runs.len())?;
        for run in &task.runs {
            run_inputs(&mut hash, run)?;
        }
        hash_value(&mut hash, &task.plans.len())?;
        for plan in &task.plans {
            hash_value(&mut hash, &(&plan.id, &plan.reviews))?;
        }
        hash_value(&mut hash, &task.artifacts.len())?;
        for artifact in &task.artifacts {
            hash_value(
                &mut hash,
                &(
                    &artifact.run,
                    artifact.kind,
                    &artifact.title,
                    &artifact.path,
                    &artifact.sha256,
                    artifact.added,
                    artifact.tier(),
                    artifact.compressed_bytes.is_some(),
                    artifact.purged_at,
                ),
            )?;
        }
    }
    Ok((hash.finalize().into(), count(Work::ArtifactStat) - stats))
}

/// Artifacts enter the digest through their recorded identity, tier, compression and purge
/// state, never their file metadata: snapshots are immutable once recorded, and an explicit
/// reindex re-reads the files themselves.
pub(crate) fn fingerprint(ctx: &Ctx, tasks: &[Task]) -> Result<String> {
    corral_core::work::record(corral_core::work::Work::FingerprintPass, 1);
    let mut hash = Sha256::new();
    // Fixed partitions keep the persisted digest independent of CPU availability.
    // Bump when sources/extraction semantics change, independently of cache format.
    hash.update(b"content-refresh-v4");
    std::thread::scope(|scope| -> Result<()> {
        let workers: Vec<_> = tasks
            .chunks(tasks.len().div_ceil(8).max(1))
            .map(|tasks| scope.spawn(move || task_inputs(tasks)))
            .collect();
        for worker in workers {
            let (digest, stats) = worker
                .join()
                .map_err(|_| anyhow::anyhow!("search input fingerprint worker panicked"))??;
            // Work counters are per thread; the workers' stats belong to this pass.
            corral_core::work::record(corral_core::work::Work::ArtifactStat, stats);
            hash.update(digest);
        }
        Ok(())
    })?;
    file_inputs(&mut hash, &ctx.store.dir().join("events.jsonl"))?;
    Ok(format!("{:x}", hash.finalize()))
}

/// Refreshes the index using a caller-owned, current task listing.
///
/// The caller must supply current tasks; no task, artifact, event, or index contents
/// are read on an unchanged refresh. Missing indexes are always rebuilt.
pub fn refresh_search_index_with_tasks(ctx: &Ctx, tasks: &[Task]) -> Result<()> {
    let _storage = crate::artifact_storage::storage_lock(ctx)?;
    let path = ctx.store.state_dir().join("content-search.fingerprint");
    let inputs = fingerprint(ctx, tasks)?;
    let index = ctx.store.state_dir().join("content-search.json");
    // Another snapshot writer (including explicit reindex) invalidates our observation.
    let current = format!("{inputs}\n{}", file_stamp(&index)?);
    let previous = match fs::read_to_string(&path) {
        Ok(previous) => Some(previous),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("reading search input fingerprint"),
    };
    if previous.as_deref() == Some(&current) && index.is_file() {
        return Ok(());
    }
    crate::content_index::refresh_snapshot(ctx, tasks)?;
    Ok(())
}

/// Successful refreshes and reindexes publish while holding the index lock.
pub(crate) fn record_build(ctx: &Ctx, inputs: &str) -> Result<()> {
    let path = ctx.store.state_dir().join("content-search.fingerprint");
    let index = ctx.store.state_dir().join("content-search.json");
    let built = format!("{inputs}\n{}", file_stamp(&index)?);
    crate::artifact_storage::replace(&path, built.as_bytes())
}

/// Refreshes the rebuildable content index without changing tasks or artifacts.
pub fn refresh_search_index(ctx: &Ctx) -> Result<()> {
    refresh_search_index_with_tasks(ctx, &ctx.store.list()?)
}

/// Observe before listing tasks, so sync can detect its own and concurrent writes.
pub(crate) fn task_stamp(ctx: &Ctx) -> Result<String> {
    let mut paths = fs::read_dir(ctx.store.dir().join("tasks"))
        .context("listing search task inputs")?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .context("reading search task entries")?;
    paths.retain(|path| path.extension().is_some_and(|ext| ext == "md"));
    paths.sort();
    let mut hash = Sha256::new();
    for path in paths {
        hash_value(&mut hash, &path)?;
        file_inputs(&mut hash, &path)?;
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) fn refresh_after_sync(ctx: &Ctx, tasks: &[Task], before: &str) -> Result<()> {
    if task_stamp(ctx)? == before {
        refresh_search_index_with_tasks(ctx, tasks)
    } else {
        refresh_search_index(ctx)
    }
}
