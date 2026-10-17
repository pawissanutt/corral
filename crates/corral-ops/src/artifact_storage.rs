//! Snapshot bytes stay at their original paths; encoding is described by artifact metadata.
use std::collections::HashSet;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::{Artifact, ArtifactTier, PinRef, PinTarget};
use flate2::{Compression, read::MultiGzDecoder, write::GzEncoder};

use crate::Ctx;

pub(crate) fn storage_lock(ctx: &Ctx) -> Result<fs::File> {
    let dir = ctx.store.dir().join("lock");
    fs::create_dir_all(&dir).context("creating artifact lock directory")?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("artifact-storage.lock"))
        .context("opening artifact storage lock")?;
    file.lock().context("locking artifact storage")?;
    Ok(file)
}

pub(crate) fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("artifact path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).context("creating artifact replacement")?;
    temp.write_all(bytes)
        .context("writing artifact replacement")?;
    temp.as_file()
        .sync_all()
        .context("syncing artifact replacement")?;
    temp.persist(path)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

pub(crate) fn link_replace(source: &Path, dest: &Path) -> Result<()> {
    let parent = dest.parent().context("artifact path has no parent")?;
    fs::create_dir_all(parent).context("creating snapshot directory")?;
    let temp = tempfile::NamedTempFile::new_in(parent).context("creating link replacement")?;
    let path = temp.into_temp_path();
    fs::remove_file(&path).context("removing temporary placeholder")?;
    fs::hard_link(source, &path).context("linking identical artifact")?;
    fs::rename(&path, dest).context("installing identical artifact")?;
    Ok(())
}

pub(crate) fn gzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).context("compressing artifact")?;
    encoder.finish().context("finishing artifact compression")
}

fn stub(a: &Artifact) -> Vec<u8> {
    format!("Artifact content purged\n\nTitle: {}\nPath: {}\nOriginal bytes: {}\nSHA-256: {}\nPurged at: {}\n",
        a.title, a.path, a.bytes, a.sha256, a.purged_at.map(|t| t.to_string()).unwrap_or_default()).into_bytes()
}

/// Opens a snapshot as a streaming reader, decoding metadata-described gzip.
/// Purged snapshots yield their text stub without opening the removed file.
///
/// # Errors
///
/// Returns an error if the snapshot cannot be opened or is not a regular file.
/// Gzip corruption and subsequent I/O errors are returned when reading the stream.
pub fn artifact_reader(ctx: &Ctx, a: &Artifact) -> Result<Box<dyn Read>> {
    if a.purged_at.is_some() {
        return Ok(Box::new(Cursor::new(stub(a))));
    }
    let path = ctx.store.dir().join(&a.path);
    let meta =
        fs::metadata(&path).with_context(|| format!("inspecting artifact {}", path.display()))?;
    if !meta.is_file() {
        bail!("artifact is not a regular file: {}", path.display());
    }
    // Nonblocking open prevents a replacement FIFO from hanging between the metadata
    // check and open. Check the retained handle as well before reading any content.
    let fd = rustix::fs::open(
        &path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .with_context(|| format!("opening artifact {}", path.display()))?;
    let file = fs::File::from(fd);
    if !file
        .metadata()
        .with_context(|| format!("inspecting opened artifact {}", path.display()))?
        .is_file()
    {
        bail!("artifact is not a regular file: {}", path.display());
    }
    if a.compressed_bytes.is_some() {
        Ok(Box::new(MultiGzDecoder::new(file)))
    } else {
        Ok(Box::new(file))
    }
}

/// Decodes a snapshot, or returns a meaningful text stub when retention purged its contents.
pub fn artifact_read(ctx: &Ctx, a: &Artifact) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    artifact_reader(ctx, a)?
        .read_to_end(&mut decoded)
        .context("reading decoded artifact")?;
    Ok(decoded)
}

/// Returns an opener-ready path, preserving extensions and the run's relative asset layout.
pub fn artifact_path(ctx: &Ctx, a: &Artifact) -> Result<PathBuf> {
    let parent = Path::new(&a.path).parent();
    let report_assets = Path::new(&a.path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "html" | "htm" | "svg"))
        && ctx
            .store
            .list()?
            .iter()
            .flat_map(|t| &t.artifacts)
            .any(|s| {
                s.purged_at.is_none()
                    && s.source != a.source
                    && Path::new(&s.path).parent() == parent
            });
    if a.purged_at.is_none() && a.compressed_bytes.is_none() && !report_assets {
        return Ok(ctx.store.dir().join(&a.path));
    }
    let cache = ctx.store.state_dir().join("artifact-decoded");
    if a.purged_at.is_some() {
        let path = cache.join(&a.path).with_extension("purged.txt");
        replace(&path, &stub(a))?;
        return Ok(path);
    }
    for task in ctx.store.list()? {
        for sibling in task
            .artifacts
            .iter()
            .filter(|s| Path::new(&s.path).parent() == parent)
        {
            if sibling.purged_at.is_some() {
                continue;
            }
            let path = cache.join(&sibling.path);
            replace(&path, &artifact_read(ctx, sibling)?)?;
            // Reports refer to source-relative assets, including subdirectories and ../.
            // Mirror only normal source components; never let source metadata escape the cache.
            if let Some(relative) = source_layout(&sibling.source) {
                let source_root = cache
                    .join(parent.context("artifact has no run directory")?)
                    .join("sources");
                link_replace(&path, &source_root.join(relative))?;
            }
        }
    }
    match source_layout(&a.source) {
        Some(relative) => {
            let path = cache
                .join(parent.context("artifact has no run directory")?)
                .join("sources")
                .join(relative);
            // Multiple snapshots can share a source name; the requested revision wins.
            link_replace(&cache.join(&a.path), &path)?;
            Ok(path)
        }
        None => Ok(cache.join(&a.path)),
    }
}

pub(crate) fn source_layout(source: &str) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for component in Path::new(source).components() {
        match component {
            std::path::Component::Normal(name) => relative.push(name),
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {}
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => return None,
        }
    }
    (!relative.as_os_str().is_empty()).then_some(relative)
}

/// Changes the explicit tier without changing a snapshot or its stable reference.
pub fn set_artifact_tier(ctx: &Ctx, reference: &str, tier: ArtifactTier) -> Result<Artifact> {
    let _lock = storage_lock(ctx)?;
    let artifact = resolve_artifact(ctx, reference)?;
    let task = ctx
        .store
        .list()?
        .into_iter()
        .find(|t| t.artifacts.iter().any(|a| a.path == artifact.path))
        .context("artifact task not found")?;
    let mut result = None;
    ctx.store.update(&task.id, |t| {
        let a = t
            .artifacts
            .iter_mut()
            .rev()
            .find(|a| a.path == artifact.path)
            .context("artifact reference not found")?;
        a.tier_override = Some(tier);
        result = Some(a.clone());
        Ok(())
    })?;
    result.context("artifact update produced no result")
}

/// Resolves a pin number, stable task:run:file reference, or exact stored/absolute snapshot path.
/// Purged artifacts remain resolvable even though their content file is gone.
pub fn resolve_artifact(ctx: &Ctx, reference: &str) -> Result<Artifact> {
    let tasks = ctx.store.list()?;
    if let Some(a) = tasks
        .iter()
        .flat_map(|t| &t.artifacts)
        .find(|a| a.path == reference || ctx.store.dir().join(&a.path) == Path::new(reference))
    {
        return Ok(a.clone());
    }
    let pins = ctx.store.pins()?;
    let pin = reference
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|n| pins.get(n));
    if let Some(pin) = pin
        && let PinTarget::Artifact { artifact, .. } =
            corral_core::resolve_pin(pin, &tasks, ctx.store.dir())
    {
        return Ok(artifact);
    }
    let reference = pin.map_or(reference, |p| p.target.as_str());
    let PinRef::Artifact { task, run, file } = PinRef::parse(reference)? else {
        bail!("expected an artifact reference (task:run:file)");
    };
    let task = tasks
        .iter()
        .find(|t| t.id == task)
        .context("artifact task not found")?;
    let exact = task
        .artifacts
        .iter()
        .find(|a| a.run == run && corral_core::snapshot_name(a) == file);
    let a = exact
        .or_else(|| {
            task.artifacts.iter().rev().find(|a| {
                let name = corral_core::snapshot_name(a);
                a.run == run
                    && name
                        .get(..8)
                        .is_some_and(|s| s.chars().all(|c| c.is_ascii_hexdigit()))
                    && name.get(8..9) == Some("-")
                    && name.get(9..) == Some(file.as_str())
            })
        })
        .context("artifact reference not found")?;
    if pin.is_some_and(|p| p.follow == Some(corral_core::Follow::Latest)) {
        return Ok(task
            .artifacts
            .iter()
            .rev()
            .find(|other| other.title == a.title || other.source == a.source)
            .unwrap_or(a)
            .clone());
    }
    Ok(a.clone())
}

pub(crate) fn protected(ctx: &Ctx, tasks: &[corral_core::Task]) -> Result<HashSet<String>> {
    let mut paths: HashSet<String> = tasks
        .iter()
        .flat_map(|t| &t.artifacts)
        .filter(|a| {
            a.tier() == ArtifactTier::Keep || a.kind == corral_core::ArtifactKind::Screenshot
        })
        .map(|a| a.path.clone())
        .collect();
    for pin in ctx.store.pins()? {
        if let PinTarget::Artifact { artifact, .. } =
            corral_core::resolve_pin(&pin, tasks, ctx.store.dir())
        {
            paths.insert(artifact.path);
        }
        // Preserve the anchor as well as the current follow-latest target.
        let anchor = corral_core::Pin {
            follow: None,
            ..pin
        };
        if let PinTarget::Artifact { artifact, .. } =
            corral_core::resolve_pin(&anchor, tasks, ctx.store.dir())
        {
            paths.insert(artifact.path);
        }
    }
    Ok(paths)
}

pub(crate) fn snapshot(
    ctx: &Ctx,
    dest: &Path,
    bytes: &[u8],
    artifact: &Artifact,
) -> Result<Option<u64>> {
    for task in ctx.store.list()? {
        if let Some(a) = task.artifacts.iter().find(|a| {
            a.sha256 == artifact.sha256
                && a.purged_at.is_none()
                && (a.compressed_bytes.is_none() || artifact.tier() != ArtifactTier::Keep)
        }) {
            link_replace(&ctx.store.dir().join(&a.path), dest)?;
            return Ok(a.compressed_bytes);
        }
    }
    replace(dest, bytes)?;
    Ok(None)
}
