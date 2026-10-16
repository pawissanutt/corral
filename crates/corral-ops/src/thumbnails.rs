//! Derived HTML previews. Originals remain the open target; thumbnails are disposable sibling files.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use corral_core::Artifact;

use crate::Ctx;
use crate::viewers::{Host, SystemHost};

/// Whether an artifact belongs in the visual gallery.
pub fn is_visual_artifact(artifact: &Artifact) -> bool {
    artifact.purged_at.is_none()
        && matches!(
            crate::viewers::extension(Path::new(&artifact.path)).as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "html" | "htm"
        )
}

fn thumbnail_path(ctx: &Ctx, artifact: &Artifact) -> PathBuf {
    ctx.store
        .dir()
        .join(format!("{}.thumbnail.png", artifact.path))
}

/// Raster snapshots display directly; HTML displays its done-time screenshot, when available.
pub fn artifact_thumbnail(ctx: &Ctx, artifact: &Artifact) -> Result<Option<PathBuf>> {
    if !is_visual_artifact(artifact) {
        return Ok(None);
    }
    if matches!(
        crate::viewers::extension(Path::new(&artifact.path)).as_str(),
        "html" | "htm"
    ) {
        let path = thumbnail_path(ctx, artifact);
        return Ok(path.is_file().then_some(path));
    }
    crate::artifact_path(ctx, artifact).map(Some)
}

fn chrome() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CORRAL_CHROME").filter(|p| !p.is_empty()) {
        return Ok(path.into());
    }
    ["google-chrome", "chromium", "chromium-browser"]
        .into_iter()
        .find(|name| SystemHost.on_path(name))
        .map(PathBuf::from)
        .context("HTML thumbnail needs Chrome/Chromium on PATH or CORRAL_CHROME")
}

fn capture(ctx: &Ctx, artifact: &Artifact, browser: &Path) -> Result<()> {
    let target = thumbnail_path(ctx, artifact);
    if target.is_file() {
        return Ok(());
    }
    // A fresh profile never reads the user's browser state. Keep rendering tied to the immutable
    // snapshot, not a worktree file that may subsequently change or disappear.
    let profile = tempfile::tempdir().context("creating thumbnail browser profile")?;
    let snapshot = crate::artifact_path(ctx, artifact)?;
    let output = profile.path().join("thumbnail.png");
    let args = [
        "--headless".to_string(),
        "--disable-gpu".to_string(),
        "--disable-background-networking".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--hide-scrollbars".to_string(),
        "--window-size=1200,800".to_string(),
        "--timeout=10000".to_string(),
        format!("--user-data-dir={}", profile.path().display()),
        format!("--screenshot={}", output.display()),
        crate::viewers::file_url(&snapshot),
    ];
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::harness::run_capture(browser, &args, Duration::from_secs(20))
        .context("capturing HTML thumbnail")?;
    let metadata = std::fs::metadata(&output).context("browser did not produce a thumbnail")?;
    if metadata.len() > ctx.store.config.artifacts.max_bytes {
        bail!("HTML thumbnail exceeds artifact max_bytes");
    }
    let bytes = std::fs::read(&output).context("reading HTML thumbnail")?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        bail!("browser output is not a PNG thumbnail");
    }
    // Use the store's atomic writer so a gallery cannot decode a partial image.
    let parent = target
        .parent()
        .context("thumbnail has no parent directory")?;
    let mut pending =
        tempfile::NamedTempFile::new_in(parent).context("creating atomic thumbnail file")?;
    pending
        .write_all(&bytes)
        .context("writing HTML thumbnail")?;
    pending.persist(&target).context("saving HTML thumbnail")?;
    Ok(())
}

/// Captures every HTML artifact in the finishing run, including files explicitly attached before done.
/// Missing browser or failed renders are reported beside the artifact, never a fabricated preview.
pub fn capture_html_thumbnails(ctx: &Ctx, id: &str) -> Result<usize> {
    let task = ctx.store.get(id)?;
    let Some(run) = task.current_run() else {
        return Ok(0);
    };
    let html: Vec<_> = task
        .artifacts
        .iter()
        .filter(|a| {
            a.run == run.id
                && a.purged_at.is_none()
                && matches!(
                    crate::viewers::extension(Path::new(&a.path)).as_str(),
                    "html" | "htm"
                )
        })
        .collect();
    if html.is_empty() {
        return Ok(0);
    }
    let browser = match chrome() {
        Ok(browser) => browser,
        Err(error) => {
            crate::messages::report(Some(&task.id), "HTML thumbnail", format!("{error:#}"));
            return Ok(0);
        }
    };
    let mut captured = 0;
    for artifact in html {
        match capture(ctx, artifact, &browser) {
            Ok(()) => captured += 1,
            Err(error) => crate::messages::report(
                Some(&task.id),
                "HTML thumbnail",
                format!("{}: {error:#}", artifact.title),
            ),
        }
    }
    Ok(captured)
}

#[cfg(test)]
mod tests;
