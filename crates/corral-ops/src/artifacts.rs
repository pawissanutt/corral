use std::path::Path;

use anyhow::bail;
use anyhow::{Context, Result};
use corral_core::{Artifact, ArtifactKind};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::Ctx;
use crate::agent::effects::{self, Receipted};
use corral_core::agent::{RequestOrigin, RequestReceipt};

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
    add_artifact_inner(ctx, id, Target::Current, path, kind, title, None, None).map(|(a, _)| a)
}

/// An assistant's scratch promotion attaches the spike diff to the Backlog task it just filed;
/// the shell's session-only artifact rule still applies to every other task artifact.
pub(crate) fn add_promotion_artifact(
    ctx: &Ctx,
    id: &str,
    path: &Path,
    title: &str,
) -> Result<Artifact> {
    let kind = Some(ArtifactKind::Diff);
    add_artifact_inner(
        ctx,
        id,
        Target::Promotion,
        path,
        kind,
        Some(title),
        None,
        None,
    )
    .map(|(a, _)| a)
}

/// Which run an artifact attaches to.
#[derive(Debug, Clone, Copy)]
enum Target<'a> {
    /// The current run, or an open plan run.
    Current,
    /// This run, which must still be current; otherwise `stale_retry`.
    Expected(&'a str),
    /// This run, current or historical: an owed effect of a named container.
    Named(&'a str),
    /// The current run of a task an assistant just filed by promoting its scratch.
    Promotion,
}

/// Attaches to the named run even after a newer run started; never to the current one.
pub(crate) fn add_artifact_to_run(
    ctx: &Ctx,
    id: &str,
    run: &str,
    path: &Path,
    kind: Option<ArtifactKind>,
    title: Option<&str>,
) -> Result<Artifact> {
    add_artifact_inner(ctx, id, Target::Named(run), path, kind, title, None, None).map(|(a, _)| a)
}

/// The Pi-native `artifact` op: attaches to the requester's run (a reviewer's parent run)
/// and commits the receipt with the artifact entry; the blob stays content-deduplicated.
pub fn add_artifact_request(
    ctx: &Ctx,
    id: &str,
    path: &Path,
    kind: Option<ArtifactKind>,
    title: Option<&str>,
    origin: &RequestOrigin,
) -> Result<Receipted> {
    let run = origin
        .identity
        .run
        .as_deref()
        .map_or(Target::Current, Target::Expected);
    effects::swept(ctx, origin, effects::Home::of(origin), || {
        add_artifact_inner(ctx, id, run, path, kind, title, Some(origin), None).map(|(_, r)| r)
    })
}

pub(crate) fn add_artifact_for_run(
    ctx: &Ctx,
    id: &str,
    run: &str,
    path: &Path,
    kind: Option<ArtifactKind>,
    title: Option<&str>,
) -> Result<Artifact> {
    add_artifact_inner(
        ctx,
        id,
        Target::Expected(run),
        path,
        kind,
        title,
        None,
        None,
    )
    .map(|(a, _)| a)
}

/// Attach an already frozen transcript while preserving its original source path.
pub(crate) fn add_transcript_bytes(
    ctx: &Ctx,
    id: &str,
    run: &str,
    path: &Path,
    bytes: &[u8],
    title: &str,
) -> Result<Artifact> {
    add_artifact_inner(
        ctx,
        id,
        Target::Named(run),
        path,
        Some(ArtifactKind::Log),
        Some(title),
        None,
        Some(bytes),
    )
    .map(|(a, _)| a)
}

#[allow(clippy::too_many_arguments)]
fn add_artifact_inner(
    ctx: &Ctx,
    id: &str,
    target: Target<'_>,
    path: &Path,
    kind: Option<ArtifactKind>,
    title: Option<&str>,
    origin: Option<&RequestOrigin>,
    frozen: Option<&[u8]>,
) -> Result<(Artifact, Option<RequestReceipt>)> {
    if !matches!(target, Target::Promotion) {
        crate::assistant_artifacts::check_owner(None)?;
    }
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
    let bytes = match frozen {
        Some(bytes) => bytes.to_vec(),
        None => std::fs::read(path).with_context(|| format!("reading {}", path.display()))?,
    };
    anyhow::ensure!(
        u64::try_from(bytes.len())? <= max,
        "artifact exceeds max_bytes {max}"
    );
    let sha = sha256_hex(&bytes);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .with_context(|| format!("artifact {} has no file name", path.display()))?;
    let source = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let id = corral_core::Store::normalize_id(id)?;
    ctx.store.head(&id)?;
    let storage = crate::artifact_storage::storage_lock(ctx)?;
    // The blob and this task's reference to it commit first; attaching then only adds rows.
    let created = crate::artifact_storage::store_blob(ctx, &id, &sha, &bytes)?;
    let attached = attach(
        ctx,
        &id,
        target,
        &bytes,
        &sha,
        (&file_name, &source),
        kind,
        title,
        origin,
        frozen.is_some(),
    );
    let (artifact, receipt) = match attached {
        Ok(attached) => attached,
        Err(error) => {
            // Only a reference no committed live artifact holds may go.
            if created && let Err(cleanup) = drop_unheld_reference(ctx, &id, &sha) {
                return Err(
                    cleanup.context(format!("dropping the blob reference after: {error:#}"))
                );
            }
            return Err(error);
        }
    };
    drop(storage);
    // The index is a rebuildable cache: failing to update it never undoes the attachment.
    if origin.is_none()
        && let Err(error) = crate::content_index::index_artifact(ctx, &id, &artifact)
    {
        crate::messages::report(
            Some(&id),
            "search index",
            format!("indexing {}: {error:#}", artifact.path),
        );
    }
    Ok((artifact, receipt))
}

fn drop_unheld_reference(ctx: &Ctx, id: &str, sha: &str) -> Result<()> {
    let held = ctx
        .store
        .get(id)?
        .artifacts
        .iter()
        .any(|a| a.sha256 == sha && a.purged_at.is_none());
    if !held {
        ctx.store.delete_record(
            crate::artifact_storage::ARTIFACT_BLOBS,
            &crate::artifact_storage::blob_key(id, sha),
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn attach(
    ctx: &Ctx,
    id: &str,
    target: Target<'_>,
    bytes: &[u8],
    sha: &str,
    (file_name, source): (&str, &Path),
    kind: Option<ArtifactKind>,
    title: Option<&str>,
    origin: Option<&RequestOrigin>,
    transcript: bool,
) -> Result<(Artifact, Option<RequestReceipt>)> {
    if origin.is_none()
        && let Some(attached) = attach_head(
            ctx,
            id,
            target,
            bytes,
            sha,
            (file_name, source),
            kind,
            title,
            transcript,
        )?
    {
        return Ok((attached, None));
    }
    let mut result: Option<(Artifact, bool)> = None;
    let mut receipt = None;
    ctx.store.update(id, |t| {
        if let Some(origin) = origin {
            effects::recheck(&ctx.store, t, origin, &effects::Home::of(origin))?;
        }
        let run = match target {
            Target::Expected(run)
                if t.current_run().is_none_or(|r| r.id != run)
                    && t.plan_runs.last().is_none_or(|r| r.id != run) =>
            {
                bail!("stale_retry: run changed before attaching artifact");
            }
            Target::Named(run) if !t.runs.iter().any(|r| r.id == run) => {
                bail!("run {run} does not exist on {}", t.id);
            }
            Target::Expected(run) | Target::Named(run) => run.to_string(),
            Target::Current | Target::Promotion => t
                .plan_runs
                .last()
                .filter(|r| r.state != corral_core::PlanRunState::Approved)
                .map(|r| r.id.clone())
                .or_else(|| t.current_run().map(|r| r.id.clone()))
                .unwrap_or_else(|| "R0".to_string()),
        };
        if let Some(index) = t
            .artifacts
            .iter()
            .position(|a| a.run == run && a.sha256 == sha)
        {
            let existing = &mut t.artifacts[index];
            let revived = existing.purged_at.is_some();
            if revived {
                existing.compressed_bytes = None;
                existing.purged_at = None;
            }
            let existing = existing.clone();
            crate::artifact_storage::record_keys(ctx, t, index, false)?;
            crate::pm_next::observe::load_checks::published(ctx, t, index, false)?;
            if let Some(origin) = origin {
                receipt = Some(artifact_receipt(t, origin, &existing, revived)?);
            }
            if revived && origin.is_none() {
                buffer_artifact_event(ctx, &t.id, &existing)?;
            }
            result = Some((existing, revived));
            return Ok(());
        }
        let snap_name = format!("{}-{file_name}", &sha[..8]);
        let artifact = Artifact {
            run: run.clone(),
            kind: kind.unwrap_or_else(|| ArtifactKind::guess(file_name)),
            title: title
                .map(str::to_string)
                .unwrap_or_else(|| file_name.to_string()),
            path: format!("artifacts/{}/{run}/{snap_name}", t.id),
            source: source.to_string_lossy().into_owned(),
            sha256: sha.to_string(),
            bytes: u64::try_from(bytes.len()).context("artifact length overflow")?,
            added: corral_core::clock::now(),
            tier_override: None,
            compressed_bytes: None,
            purged_at: None,
        };
        if artifact.kind == ArtifactKind::Diff {
            let superseded: Vec<usize> = t
                .artifacts
                .iter()
                .enumerate()
                .filter(|(_, a)| {
                    a.run == run && a.kind == ArtifactKind::Diff && a.tier_override.is_none()
                })
                .map(|(i, _)| i)
                .collect();
            for i in superseded {
                t.artifacts[i].tier_override = Some(corral_core::ArtifactTier::Transient);
                crate::artifact_storage::record_keys(ctx, t, i, false)?;
            }
        }
        t.artifacts.push(artifact.clone());
        crate::artifact_storage::record_keys(ctx, t, t.artifacts.len() - 1, true)?;
        crate::pm_next::observe::load_checks::published(ctx, t, t.artifacts.len() - 1, true)?;
        if let Some(origin) = origin {
            receipt = Some(artifact_receipt(t, origin, &artifact, true)?);
            effects::failpoint("swept_before_commit");
        }
        if origin.is_none() {
            buffer_artifact_event(ctx, &t.id, &artifact)?;
        }
        result = Some((artifact, true));
        Ok(())
    })?;
    let (artifact, _) = result.context("artifact update produced no result")?;
    Ok((artifact, receipt))
}

/// [`attach`] of a registration without a request origin, as a head update: the target run and
/// its own registrations are read by key, never the task's other artifacts.
#[allow(clippy::too_many_arguments)]
fn attach_head(
    ctx: &Ctx,
    id: &str,
    target: Target<'_>,
    bytes: &[u8],
    sha: &str,
    (file_name, source): (&str, &Path),
    kind: Option<ArtifactKind>,
    title: Option<&str>,
    transcript: bool,
) -> Result<Option<Artifact>> {
    let mut result = None;
    ctx.store.try_update_head(id, |task| {
        result = Some(attach_to_head(
            ctx,
            task,
            target,
            bytes.len(),
            sha,
            (file_name, source),
            kind,
            title,
            transcript,
            None,
        )?);
        Ok(true)
    })?;
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn attach_to_head(
    ctx: &Ctx,
    t: &mut corral_core::TaskHeadMut,
    target: Target<'_>,
    bytes: usize,
    sha: &str,
    (file_name, source): (&str, &Path),
    kind: Option<ArtifactKind>,
    title: Option<&str>,
    transcript: bool,
    cached: Option<&mut Vec<(usize, Artifact)>>,
) -> Result<Artifact> {
    use corral_core::TaskRef as _;
    let kind = kind.unwrap_or_else(|| ArtifactKind::guess(file_name));
    let current = t.current_run_head()?.map(|r| r.id.clone());
    let planning = t.last_plan_run_head()?.map(|r| (r.id.clone(), r.state));
    let run = match target {
        Target::Expected(run)
            if current.as_deref() != Some(run)
                && planning.as_ref().is_none_or(|(id, _)| id != run) =>
        {
            bail!("stale_retry: run changed before attaching artifact");
        }
        Target::Named(run) if t.run_head(run)?.is_none() => {
            bail!("run {run} does not exist on {}", t.id);
        }
        Target::Expected(run) | Target::Named(run) => run.to_string(),
        Target::Current | Target::Promotion => planning
            .filter(|(_, state)| *state != corral_core::PlanRunState::Approved)
            .map(|(id, _)| id)
            .or(current)
            .unwrap_or_else(|| "R0".to_string()),
    };
    let before = t
        .head()
        .current_run
        .as_ref()
        .filter(|r| r.id == run)
        .map(|_| t.head().summary.current_artifacts.accepted_proof.clone());
    let mut loaded;
    let rows = match cached {
        Some(rows) => rows,
        None => {
            loaded = if transcript {
                t.head()
                    .artifact_registration_by_sha(&run, sha)?
                    .into_iter()
                    .collect()
            } else {
                t.head().artifact_rows_of(&run)?
            };
            &mut loaded
        }
    };
    if let Some((index, mut existing)) = rows.iter().find(|(_, a)| a.sha256 == sha).cloned() {
        if existing.purged_at.is_some() {
            existing.compressed_bytes = None;
            existing.purged_at = None;
            t.edit_artifact(index, |a| {
                *a = existing.clone();
                Ok(())
            })?;
            buffer_artifact_event(ctx, &t.id, &existing)?;
        }
        rows.iter_mut()
            .filter(|(i, _)| *i == index)
            .for_each(|(_, a)| *a = existing.clone());
        crate::artifact_storage::record_row_keys(ctx, &t.id, &existing, index, false)?;
        if transcript {
            crate::pm_next::observe::load_checks::published_transcript(
                ctx,
                t.head(),
                &existing,
                before.as_ref(),
            )?;
        } else {
            crate::pm_next::observe::load_checks::published_rows(ctx, &t.id, rows, index, false)?;
        }
        return Ok(existing);
    }
    let index = usize::try_from(t.head().summary.artifacts)?;
    let snap_name = format!("{}-{file_name}", &sha[..8]);
    let artifact = Artifact {
        run: run.clone(),
        kind,
        title: title
            .map(str::to_string)
            .unwrap_or_else(|| file_name.to_string()),
        path: format!("artifacts/{}/{run}/{snap_name}", t.id),
        source: source.to_string_lossy().into_owned(),
        sha256: sha.to_string(),
        bytes: u64::try_from(bytes).context("artifact length overflow")?,
        added: corral_core::clock::now(),
        tier_override: None,
        compressed_bytes: None,
        purged_at: None,
    };
    if artifact.kind == ArtifactKind::Diff {
        for (index, earlier) in rows.iter_mut() {
            if earlier.kind == ArtifactKind::Diff && earlier.tier_override.is_none() {
                earlier.tier_override = Some(corral_core::ArtifactTier::Transient);
                t.edit_artifact(*index, |a| {
                    *a = earlier.clone();
                    Ok(())
                })?;
                crate::artifact_storage::record_row_keys(ctx, &t.id, earlier, *index, false)?;
            }
        }
    }
    t.add_artifact(artifact.clone())?;
    crate::artifact_storage::record_row_keys(ctx, &t.id, &artifact, index, true)?;
    rows.push((index, artifact.clone()));
    if transcript {
        crate::pm_next::observe::load_checks::published_transcript(
            ctx,
            t.head(),
            &artifact,
            before.as_ref(),
        )?;
    } else {
        crate::pm_next::observe::load_checks::published_rows(ctx, &t.id, rows, index, true)?;
    }
    buffer_artifact_event(ctx, &t.id, &artifact)?;
    Ok(artifact)
}

/// Called inside the same task operation that publishes the artifact registration.
fn buffer_artifact_event(ctx: &Ctx, task: &str, artifact: &Artifact) -> Result<()> {
    effects::failpoint("artifact_event");
    ctx.store
        .log_event(Some(task), "artifact", artifact_detail(artifact))
}

fn artifact_detail(artifact: &Artifact) -> serde_json::Value {
    json!({ "run": artifact.run, "kind": artifact.kind.as_str(), "title": artifact.title, "path": artifact.path })
}

fn artifact_receipt(
    task: &mut corral_core::Task,
    origin: &RequestOrigin,
    artifact: &Artifact,
    added: bool,
) -> Result<RequestReceipt> {
    let mut result = json!({
        "path": artifact.path, "sha256": artifact.sha256, "run": artifact.run, "added": added,
    });
    let mut owed = vec!["effect:content_index"];
    if added {
        result["event"] = effects::event("artifact", artifact_detail(artifact), origin);
        owed.insert(0, "effect:event");
    }
    let effect = Some(("artifact", artifact.path.clone()));
    let commit = effects::Commit {
        result,
        effect,
        owed,
    };
    effects::record(task, origin, &effects::Home::of(origin), commit)
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

mod scan;
pub use scan::scan_artifacts;

/// Every artifact of every task, newest first.
pub fn all_artifacts(ctx: &Ctx) -> Result<Vec<ArtifactRow>> {
    let mut rows: Vec<ArtifactRow> = ctx
        .store
        .artifact_owners()?
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
