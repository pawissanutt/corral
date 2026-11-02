//! Automatic artifact discovery publishes one atomic set through the v2 blob/row APIs.
use super::*;

/// Finds changed and ignored evidence files matching the configured artifact globs.
/// Blob snapshots precede one registration transaction, so interrupted scans are retryable.
pub fn scan_artifacts(ctx: &Ctx, id: &str) -> Result<Vec<Artifact>> {
    let task = ctx.store.head(id)?;
    let Some(run) = task.current_run.as_deref() else {
        return Ok(Vec::new());
    };
    let stat = corral_git::diff_stat(&run.worktree, &run.base_sha)?;
    let cfg = &ctx.store.config.artifacts;
    let paths: Vec<_> = stat
        .files
        .iter()
        .map(|file| file.path.clone())
        .chain(corral_git::ignored_artifact_scratch(&run.worktree)?)
        .filter(|path| cfg.globs.iter().any(|g| glob_match(g, path)))
        .collect();
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    crate::assistant_artifacts::check_owner(None)?;
    let storage = crate::artifact_storage::storage_lock(ctx)?;
    let mut snapshots = Vec::new();
    for path in paths {
        let full = run.worktree.join(path);
        match std::fs::metadata(&full) {
            Ok(m) if m.is_file() && m.len() <= cfg.max_bytes => {}
            _ => continue,
        }
        let bytes = std::fs::read(&full).with_context(|| format!("reading {}", full.display()))?;
        anyhow::ensure!(
            bytes.len() as u64 <= cfg.max_bytes,
            "artifact {} exceeds max_bytes",
            full.display()
        );
        let sha = sha256_hex(&bytes);
        crate::artifact_storage::store_blob(ctx, id, &sha, &bytes)?;
        effects::failpoint("artifact_scan_snapshot");
        snapshots.push((full, bytes.len(), sha));
    }
    let mut known: std::collections::HashSet<_> = task
        .artifacts_of(&run.id)?
        .into_iter()
        .map(|a| a.path)
        .collect();
    let mut added = Vec::new();
    let mut published = Vec::new();
    ctx.store.try_update_head(id, |current| {
        anyhow::ensure!(
            current
                .head()
                .current_run
                .as_ref()
                .is_some_and(|r| r.id == run.id),
            "stale_retry: run changed during artifact scan"
        );
        let mut rows = current.head().artifact_rows_of(&run.id)?;
        for (full, bytes, sha) in &snapshots {
            let file_name = full
                .file_name()
                .context("artifact has no file name")?
                .to_string_lossy();
            let source = std::fs::canonicalize(full).unwrap_or_else(|_| full.clone());
            let artifact = attach_to_head(
                ctx,
                current,
                Target::Expected(&run.id),
                *bytes,
                sha,
                (&file_name, &source),
                None,
                None,
                false,
                Some(&mut rows),
            )?;
            if known.insert(artifact.path.clone()) {
                added.push(artifact.clone());
            }
            published.push(artifact);
        }
        Ok(!snapshots.is_empty())
    })?;
    drop(storage);
    if let Err(error) = crate::content_index::index_artifacts(ctx, id, &published) {
        crate::messages::report(
            Some(id),
            "search index",
            format!("indexing scanned artifacts: {error:#}"),
        );
    }
    Ok(added)
}
