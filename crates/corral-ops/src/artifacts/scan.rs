//! Automatic artifact discovery prepares slow I/O before one atomic metadata publication.
use super::*;

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;

struct Prepared {
    bytes: usize,
    sha: String,
    file_name: String,
    source: std::path::PathBuf,
    blob: corral_core::blob::PutBlob,
}

/// Finds changed and ignored evidence files matching the configured artifact globs.
/// Blob preparation holds no task/record view; references and registrations commit together.
pub fn scan_artifacts(ctx: &Ctx, id: &str) -> Result<Vec<Artifact>> {
    let (run, mut known) = {
        let task = ctx.store.head(id)?;
        let Some(run) = task.current_run.as_deref().cloned() else {
            return Ok(Vec::new());
        };
        let known: std::collections::HashSet<_> = task
            .artifacts_of(&run.id)?
            .into_iter()
            .map(|a| a.path)
            .collect();
        (run, known)
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
    let mut extracted = std::collections::BTreeMap::new();
    let mut extracted_bytes = 0;
    let mut prepared = Vec::new();
    for path in &paths {
        let full = run.worktree.join(path);
        match std::fs::metadata(&full) {
            Ok(m) if m.is_file() && m.len() <= cfg.max_bytes => {}
            _ => continue,
        }
        let _output_consumer = crate::storage_gc::outputs::protect_artifact_source(ctx, &full)?;
        let bytes = std::fs::read(&full).with_context(|| format!("reading {}", full.display()))?;
        anyhow::ensure!(
            bytes.len() as u64 <= cfg.max_bytes,
            "artifact {} exceeds max_bytes",
            full.display()
        );
        let sha = sha256_hex(&bytes);
        let blob = ctx.store.put_gate_blob(&bytes)?;
        // Preserve exact frozen text, bounded independently of the number of evidence files.
        let text = crate::content_index::extract_bytes(&bytes);
        if !extracted.contains_key(&sha)
            && extracted_bytes + text.text.len() <= crate::content_index::TEXT_BUDGET
        {
            extracted_bytes += text.text.len();
            extracted.insert(sha.clone(), text);
        }
        prepared.push(Prepared {
            bytes: bytes.len(),
            sha,
            file_name: full
                .file_name()
                .context("artifact has no file name")?
                .to_string_lossy()
                .into_owned(),
            source: std::fs::canonicalize(&full).unwrap_or_else(|_| full.clone()),
            blob,
        });
    }
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
        for file in prepared {
            ctx.store.stage_prepared_task_evidence(
                crate::artifact_storage::ARTIFACT_BLOBS,
                &crate::artifact_storage::blob_key(id, &file.sha),
                file.blob,
                |blob| json!({ "task": id, "sha256": file.sha, "blob": blob }),
            )?;
            effects::failpoint("artifact_scan_snapshot");
            let artifact = attach_to_head(
                ctx,
                current,
                Target::Expected(&run.id),
                file.bytes,
                &file.sha,
                (&file.file_name, &file.source),
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
        // Fold the derived cache once, not once per registration.
        if !published.is_empty()
            && let Some((index, _)) = rows.last()
        {
            crate::pm_next::observe::load_checks::published_rows(ctx, id, &rows, *index, false)?;
        }
        Ok(!published.is_empty())
    })?;
    drop(storage);
    if let Err(error) = crate::content_index::index_artifacts(ctx, id, &published, &extracted) {
        crate::messages::report(
            Some(id),
            "search index",
            format!("indexing scanned artifacts: {error:#}"),
        );
    }
    Ok(added)
}
