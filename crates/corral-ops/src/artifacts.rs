use std::collections::HashSet;
use std::path::Path;

use anyhow::bail;
use anyhow::{Context, Result};
use corral_core::{Artifact, ArtifactKind};
use jiff::Timestamp;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::Ctx;

/// One artifact with its task context, for the global finder.
#[derive(Debug, Clone)]
pub struct ArtifactRow {
    pub task_id: String,
    pub task_title: String,
    pub project: Option<String>,
    pub artifact: Artifact,
}

/// How the TUI should show an artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    /// Text to render (markdown/log/diff/plain), already truncated to 2000 lines.
    Text(String),
    /// A raster image (png/jpg/gif/webp/bmp) the TUI decodes and draws: absolute path + a one-line description.
    Image { path: String, note: String },
    /// Not renderable in the TUI (html/svg/pdf/binary): absolute path + a one-line description; the TUI offers
    /// "open" (`crate::open_path`).
    External { path: String, note: String },
}

const PREVIEW_LINES: usize = 2000;

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Snapshots `path` into artifacts/<task>/<run>/<sha8>-<file name> for the task's current run (run "R0" when
/// the task has no run) and appends it to task.artifacts. Same sha already recorded for that run -> returns the
/// existing artifact (no duplicate). Errors: missing file, directory, larger than config max_bytes.
/// Title default: file name. Logs "artifact".
pub fn add_artifact(
    ctx: &Ctx,
    id: &str,
    path: &Path,
    kind: Option<ArtifactKind>,
    title: Option<&str>,
) -> Result<Artifact> {
    add_artifact_inner(ctx, id, None, path, kind, title)
}

pub(crate) fn add_artifact_for_run(
    ctx: &Ctx,
    id: &str,
    run: &str,
    path: &Path,
    kind: Option<ArtifactKind>,
    title: Option<&str>,
) -> Result<Artifact> {
    add_artifact_inner(ctx, id, Some(run), path, kind, title)
}

fn add_artifact_inner(
    ctx: &Ctx,
    id: &str,
    expected_run: Option<&str>,
    path: &Path,
    kind: Option<ArtifactKind>,
    title: Option<&str>,
) -> Result<Artifact> {
    let _storage_lock = crate::artifact_storage::storage_lock(ctx)?;
    let meta = std::fs::metadata(path)
        .with_context(|| format!("artifact {} not found", path.display()))?;
    if !meta.is_file() {
        bail!("artifact {} is not a regular file", path.display());
    }
    let max = ctx.store.config.artifacts.max_bytes;
    if meta.len() > max {
        bail!(
            "artifact {} is {} bytes, larger than max_bytes {max}",
            path.display(),
            meta.len()
        );
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let sha = sha256_hex(&bytes);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .with_context(|| format!("artifact {} has no file name", path.display()))?;
    let source = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let mut result: Option<(Artifact, bool)> = None;
    let task = ctx.store.update(id, |t| {
        if expected_run.is_some_and(|run| {
            t.current_run().is_none_or(|r| r.id != run)
                && t.plan_runs.last().is_none_or(|r| r.id != run)
        }) {
            bail!("stale_retry: run changed before attaching artifact");
        }
        let run = expected_run
            .map(str::to_string)
            .or_else(|| {
                t.plan_runs
                    .last()
                    .filter(|r| {
                        r.state != corral_core::PlanRunState::Approved && expected_run.is_none()
                    })
                    .map(|r| r.id.clone())
                    .or_else(|| t.current_run().map(|r| r.id.clone()))
            })
            .unwrap_or_else(|| "R0".to_string());
        if let Some(existing) = t
            .artifacts
            .iter_mut()
            .find(|a| a.run == run && a.sha256 == sha)
        {
            let revived = existing.purged_at.is_some();
            if revived {
                existing.compressed_bytes = crate::artifact_storage::snapshot(
                    ctx,
                    &ctx.store.dir().join(&existing.path),
                    &bytes,
                    existing,
                )?;
                existing.purged_at = None;
            }
            result = Some((existing.clone(), revived));
            return Ok(());
        }
        let snap_name = format!("{}-{file_name}", &sha[..8]);
        let dir = ctx.store.artifacts_dir(&t.id, &run);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let dest = dir.join(&snap_name);
        let mut artifact = Artifact {
            run: run.clone(),
            kind: kind.unwrap_or_else(|| ArtifactKind::guess(&file_name)),
            title: title
                .map(str::to_string)
                .unwrap_or_else(|| file_name.clone()),
            path: format!("artifacts/{}/{run}/{snap_name}", t.id),
            source: source.to_string_lossy().into_owned(),
            sha256: sha.clone(),
            bytes: u64::try_from(bytes.len()).context("artifact length overflow")?,
            added: Timestamp::now(),
            tier_override: None,
            compressed_bytes: None,
            purged_at: None,
        };
        artifact.compressed_bytes =
            crate::artifact_storage::snapshot(ctx, &dest, &bytes, &artifact)?;
        if artifact.kind == ArtifactKind::Diff {
            for previous in t.artifacts.iter_mut().filter(|a| {
                a.run == run && a.kind == ArtifactKind::Diff && a.tier_override.is_none()
            }) {
                previous.tier_override = Some(corral_core::ArtifactTier::Transient);
            }
        }
        t.artifacts.push(artifact.clone());
        result = Some((artifact, true));
        Ok(())
    })?;
    let (artifact, added) = result.context("artifact update produced no result")?;
    if added {
        ctx.store.log_event(
            Some(&task.id),
            "artifact",
            json!({ "run": artifact.run, "kind": artifact.kind.as_str(), "title": artifact.title, "path": artifact.path }),
        )?;
    }
    crate::content_index::snapshot(ctx, &ctx.store.list()?, false)?;
    Ok(artifact)
}

fn segment_match(pat: &[u8], s: &[u8]) -> bool {
    match pat.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| segment_match(rest, &s[i..])),
        Some((c, rest)) => s.first() == Some(c) && segment_match(rest, &s[1..]),
    }
}

fn segments_match(pat: &[&str], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|i| segments_match(rest, &path[i..])),
        Some((p, rest)) => match path.split_first() {
            Some((s, prest)) => {
                segment_match(p.as_bytes(), s.as_bytes()) && segments_match(rest, prest)
            }
            None => false,
        },
    }
}

pub(crate) fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    segments_match(&pat, &segs)
}

/// Finds files in the run worktree matching config.artifacts.globs that changed since the run's base
/// (corral_git changed files incl. untracked) and adds each with `ArtifactKind::guess`. Returns the added ones.
/// Simple glob support is enough: "**" any dirs, "*" within one path segment.
pub fn scan_artifacts(ctx: &Ctx, id: &str) -> Result<Vec<Artifact>> {
    let task = ctx.store.get(id)?;
    let Some(run) = task.current_run() else {
        return Ok(Vec::new());
    };
    let known: HashSet<String> = task.artifacts.iter().map(|a| a.path.clone()).collect();
    let stat = corral_git::diff_stat(&run.worktree, &run.base_sha)?;
    let cfg = &ctx.store.config.artifacts;
    let mut added = Vec::new();
    for file in &stat.files {
        if !cfg.globs.iter().any(|g| glob_match(g, &file.path)) {
            continue;
        }
        let full = run.worktree.join(&file.path);
        match std::fs::metadata(&full) {
            Ok(m) if m.is_file() && m.len() <= cfg.max_bytes => {}
            _ => continue,
        }
        let artifact = add_artifact(ctx, &task.id, &full, None, None)?;
        if !known.contains(&artifact.path)
            && !added.iter().any(|a: &Artifact| a.path == artifact.path)
        {
            added.push(artifact);
        }
    }
    Ok(added)
}

/// Every artifact of every task, newest first.
pub fn all_artifacts(ctx: &Ctx) -> Result<Vec<ArtifactRow>> {
    let mut rows: Vec<ArtifactRow> = ctx
        .store
        .list()?
        .into_iter()
        .flat_map(|t| {
            let (id, title, project) = (t.id, t.title, t.project);
            t.artifacts.into_iter().map(move |a| ArtifactRow {
                task_id: id.clone(),
                task_title: title.clone(),
                project: project.clone(),
                artifact: a,
            })
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.artifact.added));
    Ok(rows)
}

/// Preview for an artifact: raster images -> Image; text kinds and text-looking files (utf-8, no NUL in first
/// 8 KiB) -> Text; svg, .html and binaries -> External.
pub fn artifact_preview(ctx: &Ctx, artifact: &Artifact) -> Result<Preview> {
    read_preview(ctx, artifact, Some(PREVIEW_LINES))
}

/// Like [`artifact_preview`], but a text artifact comes back whole (decoded, never cut to a line cap) so
/// readers can search and scroll the complete document.
pub fn artifact_document(ctx: &Ctx, artifact: &Artifact) -> Result<Preview> {
    read_preview(ctx, artifact, None)
}

fn read_preview(ctx: &Ctx, artifact: &Artifact, limit: Option<usize>) -> Result<Preview> {
    if artifact.purged_at.is_some() {
        return Ok(Preview::Text(String::from_utf8(crate::artifact_read(
            ctx, artifact,
        )?)?));
    }
    let lower = artifact.path.to_ascii_lowercase();
    let ext = Path::new(&lower)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let external = |note: String| {
        Ok(Preview::External {
            path: crate::artifact_path(ctx, artifact)?
                .to_string_lossy()
                .into_owned(),
            note,
        })
    };
    if matches!(ext, "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp") {
        return Ok(Preview::Image {
            note: format!("{ext} image ({} bytes)", artifact.bytes),
            path: crate::artifact_path(ctx, artifact)?
                .to_string_lossy()
                .into_owned(),
        });
    }
    if ext == "svg" || artifact.kind == ArtifactKind::Screenshot {
        return external(format!("image ({} bytes)", artifact.bytes));
    }
    if matches!(ext, "html" | "htm") {
        return external(format!("HTML document ({} bytes)", artifact.bytes));
    }
    let bytes = crate::artifact_read(ctx, artifact)?;
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        return external(format!("binary file ({} bytes)", artifact.bytes));
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return external(format!("non-UTF-8 file ({} bytes)", artifact.bytes));
    };
    let Some(limit) = limit else {
        return Ok(Preview::Text(text));
    };
    let total = text.lines().count();
    if total <= limit {
        return Ok(Preview::Text(text));
    }
    let mut out: String = text
        .lines()
        .take(limit - 1)
        .flat_map(|l| [l, "\n"])
        .collect();
    out.push_str(&format!("... (truncated, {total} lines)\n"));
    Ok(Preview::Text(out))
}
