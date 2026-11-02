//! Automatic artifact discovery publishes one atomic set through the v2 blob/row APIs.
use super::*;

/// Finds changed and ignored evidence files matching the configured artifact globs.
/// Existing evidence staging batches references and registrations in one head operation.
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
    let started = std::time::Instant::now();
    let trace = |stage: &str| {
        if std::env::var_os("CORRAL_DONE_TIMING").is_some() {
            eprintln!(
                "artifact scan {id}: {stage}: {:.3}s",
                started.elapsed().as_secs_f64()
            );
        }
    };
    let storage = crate::artifact_storage::storage_lock(ctx)?;
    let mut extracted = std::collections::BTreeMap::new();
    let mut extracted_bytes = 0;
    let mut known: std::collections::HashSet<_> = task
        .artifacts_of(&run.id)?
        .into_iter()
        .map(|a| a.path)
        .collect();
    let mut added = Vec::new();
    let mut published = Vec::new();
    let mut blob_time = std::time::Duration::ZERO;
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
        for path in &paths {
            let full = run.worktree.join(path);
            match std::fs::metadata(&full) {
                Ok(m) if m.is_file() && m.len() <= cfg.max_bytes => {}
                _ => continue,
            }
            let bytes =
                std::fs::read(&full).with_context(|| format!("reading {}", full.display()))?;
            anyhow::ensure!(
                bytes.len() as u64 <= cfg.max_bytes,
                "artifact {} exceeds max_bytes",
                full.display()
            );
            let sha = sha256_hex(&bytes);
            let blob_started = std::time::Instant::now();
            ctx.store.stage_task_evidence(
                crate::artifact_storage::ARTIFACT_BLOBS,
                &crate::artifact_storage::blob_key(id, &sha),
                &bytes,
                |blob| json!({ "task": id, "sha256": sha, "blob": blob }),
            )?;
            blob_time += blob_started.elapsed();
            effects::failpoint("artifact_scan_snapshot");
            // Index the exact frozen bytes without reopening every blob. Prepared text
            // remains bounded by the existing total index budget.
            let text = crate::content_index::extract_bytes(&bytes);
            if !extracted.contains_key(&sha)
                && extracted_bytes + text.text.len() <= crate::content_index::TEXT_BUDGET
            {
                extracted_bytes += text.text.len();
                extracted.insert(sha.clone(), text);
            }
            let file_name = full
                .file_name()
                .context("artifact has no file name")?
                .to_string_lossy();
            let source = std::fs::canonicalize(&full).unwrap_or_else(|_| full.clone());
            let artifact = attach_to_head(
                ctx,
                current,
                Target::Expected(&run.id),
                bytes.len(),
                &sha,
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
        // Fold this derived cache once from the final set instead of hashing every
        // earlier registration again after each append.
        if !published.is_empty()
            && let Some((index, _)) = rows.last()
        {
            crate::pm_next::observe::load_checks::published_rows(ctx, id, &rows, *index, false)?;
        }
        Ok(!published.is_empty())
    })?;
    if std::env::var_os("CORRAL_DONE_TIMING").is_some() {
        eprintln!(
            "artifact scan {id}: blob staging total: {:.3}s",
            blob_time.as_secs_f64()
        );
    }
    trace("snapshots and registrations");
    drop(storage);
    if let Err(error) = crate::content_index::index_artifacts(ctx, id, &published, &extracted) {
        crate::messages::report(
            Some(id),
            "search index",
            format!("indexing scanned artifacts: {error:#}"),
        );
    }
    trace("index");
    Ok(added)
}
