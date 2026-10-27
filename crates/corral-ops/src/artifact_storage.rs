//! Artifact bytes are content-addressed blobs (plan T-253 section 7): each task holds one
//! `artifact-blobs` record per distinct content, whose document references the blob. Until
//! `review-scratch` removes `artifacts_dir()`, the legacy copy at `<store dir>/<path>` is still
//! written for the readers that open it directly, and reads keep its checks: they read the
//! copy while it exists and decode the blob once it is gone. Encoding of that copy is
//! described by artifact metadata.
use std::collections::HashSet;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::{Artifact, ArtifactTier, PinRef, PinTarget, Store};
use flate2::read::MultiGzDecoder;

use crate::Ctx;

/// Records whose documents hold each task's artifact blob references, keyed `<task>/<sha256>`.
pub(crate) const ARTIFACT_BLOBS: &str = "artifact-blobs";

pub(crate) fn blob_key(task: &str, sha256: &str) -> String {
    format!("{task}/{sha256}")
}

/// Puts `bytes` as a blob and references it from `task`'s record for that content. Returns
/// whether the record is new (the caller drops it again if attaching fails).
pub(crate) fn store_blob(ctx: &Ctx, task: &str, sha256: &str, bytes: &[u8]) -> Result<bool> {
    ctx.store.put_evidence(
        ARTIFACT_BLOBS,
        &blob_key(task, sha256),
        bytes,
        |blob| serde_json::json!({ "task": task, "sha256": sha256, "blob": blob }),
    )
}

/// The task an artifact belongs to, from its `artifacts/<task>/<run>/<file>` path.
pub(crate) fn task_of(path: &str) -> Option<&str> {
    path.strip_prefix("artifacts/")?
        .split('/')
        .next()
        .filter(|task| !task.is_empty())
}

fn blob_reader(ctx: &Ctx, a: &Artifact) -> Result<Option<Box<dyn Read + Send>>> {
    match corral_core::blob_ref::BlobHash::parse(&a.sha256) {
        Ok(hash) => ctx.store.blob_reader(&hash),
        Err(_) => Ok(None),
    }
}

/// Whether the artifact's bytes can be read: its legacy copy, or else its live blob file.
pub(crate) fn content_available(ctx: &Ctx, a: &Artifact) -> Result<bool> {
    if ctx.store.dir().join(&a.path).exists() {
        return Ok(true);
    }
    let Ok(hash) = corral_core::blob_ref::BlobHash::parse(&a.sha256) else {
        return Ok(false);
    };
    let live = ctx
        .store
        .blob_row(&hash)?
        .is_some_and(|row| row.state == corral_core::backend::BlobState::Live);
    Ok(live && ctx.store.blob_store()?.path_of(&hash).is_file())
}

/// The legacy copy is missing (not replaced by something else, which stays an error).
fn absent(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            || cause
                .downcast_ref::<rustix::io::Errno>()
                .is_some_and(|e| *e == rustix::io::Errno::NOENT)
    })
}

/// Root of decoded copies handed to openers: `<cache_root>/decoded`, never under `.corral`.
pub(crate) fn decoded_root(ctx: &Ctx) -> Result<PathBuf> {
    Ok(ctx.store.cache_root()?.join("decoded"))
}

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
    use flate2::{Compression, write::GzEncoder};
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
    let meta = match fs::metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(reader) = blob_reader(ctx, a)? {
                return Ok(reader);
            }
            Err(error)
        }
        meta => meta,
    }
    .with_context(|| format!("inspecting artifact {}", path.display()))?;
    if !meta.is_file() {
        bail!("artifact is not a regular file: {}", path.display());
    }
    // Nonblocking open prevents a replacement FIFO from hanging between the metadata
    // check and open. Check the retained handle as well before reading any content.
    #[cfg(unix)]
    let file = fs::File::from(
        rustix::fs::open(
            &path,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .with_context(|| format!("opening artifact {}", path.display()))?,
    );
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        // Open the reparse point itself rather than following a swapped-in link
        // to a device or named pipe; validate the retained disk file below.
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(0x0020_0000)
            .open(&path)
            .with_context(|| format!("opening artifact {}", path.display()))?
    };
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

/// Like [`artifact_read`], but the snapshot is opened through a handle confined beneath `.corral/artifacts`:
/// a symlinked snapshot or parent directory is refused instead of followed, so the bytes always come from the
/// artifact store (the live server's read-only boundary).
pub(crate) fn artifact_read_confined(ctx: &Ctx, a: &Artifact) -> Result<Vec<u8>> {
    if a.purged_at.is_some() {
        return Ok(stub(a));
    }
    let file = match open_beneath_artifacts(ctx, &a.path) {
        Ok(file) => file,
        Err(error) if absent(&error) => {
            let Some(mut reader) = blob_reader(ctx, a)? else {
                return Err(error);
            };
            let mut decoded = Vec::new();
            reader
                .read_to_end(&mut decoded)
                .context("reading artifact blob")?;
            return Ok(decoded);
        }
        Err(error) => return Err(error),
    };
    if !file
        .metadata()
        .context("inspecting opened artifact")?
        .is_file()
    {
        bail!("artifact is not a regular file: {}", a.path);
    }
    let mut reader: Box<dyn Read> = if a.compressed_bytes.is_some() {
        Box::new(MultiGzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut decoded = Vec::new();
    reader
        .read_to_end(&mut decoded)
        .context("reading decoded artifact")?;
    Ok(decoded)
}

#[cfg(target_os = "linux")]
fn open_beneath_artifacts(ctx: &Ctx, snapshot: &str) -> Result<fs::File> {
    use rustix::fs::{Mode, OFlags, ResolveFlags};
    let rel = snapshot
        .strip_prefix("artifacts/")
        .with_context(|| format!("snapshot {snapshot} is not under artifacts/"))?;
    let root = ctx.store.dir().join("artifacts");
    let dir = rustix::fs::open(
        &root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .with_context(|| format!("opening {}", root.display()))?;
    let fd = rustix::fs::openat2(
        &dir,
        rel,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )
    .with_context(|| format!("opening {snapshot} beneath {}", root.display()))?;
    Ok(fs::File::from(fd))
}

#[cfg(not(target_os = "linux"))]
fn open_beneath_artifacts(ctx: &Ctx, snapshot: &str) -> Result<fs::File> {
    let root =
        fs::canonicalize(ctx.store.dir().join("artifacts")).context("resolving artifacts")?;
    let path = ctx.store.dir().join(snapshot);
    let mut check = root.clone();
    for part in Path::new(
        snapshot
            .strip_prefix("artifacts/")
            .context("snapshot is not under artifacts/")?,
    )
    .components()
    {
        let std::path::Component::Normal(name) = part else {
            bail!("snapshot {snapshot} leaves artifacts/");
        };
        check.push(name);
        if fs::symlink_metadata(&check)
            .with_context(|| format!("inspecting {}", check.display()))?
            .file_type()
            .is_symlink()
        {
            bail!("snapshot {snapshot} goes through a symlink");
        }
    }
    #[cfg(unix)]
    let file = fs::File::from(
        rustix::fs::open(
            &path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .with_context(|| format!("opening artifact {}", path.display()))?,
    );
    #[cfg(windows)]
    let file = {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;

        // SAFETY: the ABI follows fileapi.h; HANDLE is a pointer and DWORD is u32.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFinalPathNameByHandleW(
                file: *mut std::ffi::c_void,
                path: *mut u16,
                length: u32,
                flags: u32,
            ) -> u32;
        }

        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(0x0020_0000) // FILE_FLAG_OPEN_REPARSE_POINT
            .open(&path)
            .with_context(|| format!("opening artifact {}", path.display()))?;
        // SAFETY: file retains the handle; a null buffer requests its normalized
        // DOS path length, including the terminator.
        let length =
            unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), std::ptr::null_mut(), 0, 0) };
        if length == 0 {
            return Err(std::io::Error::last_os_error()).context("sizing opened artifact path");
        }
        let mut path = vec![0_u16; usize::try_from(length)?];
        // SAFETY: the live buffer holds length UTF-16 code units; file stays open.
        let written = unsafe {
            GetFinalPathNameByHandleW(file.as_raw_handle(), path.as_mut_ptr(), length, 0)
        };
        if written == 0 {
            return Err(std::io::Error::last_os_error()).context("resolving opened artifact path");
        }
        if written >= length {
            bail!("opened artifact path changed during resolution");
        }
        let opened = PathBuf::from(OsString::from_wide(&path[..usize::try_from(written)?]));
        if !opened.starts_with(&root)
            || file
                .metadata()
                .context("inspecting confined artifact handle")?
                .file_type()
                .is_symlink()
        {
            bail!("opened artifact leaves artifacts/ or names a symlink");
        }
        file
    };
    Ok(file)
}

/// Decodes a snapshot, or returns a meaningful text stub when retention purged its contents.
pub fn artifact_read(ctx: &Ctx, a: &Artifact) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    artifact_reader(ctx, a)?
        .read_to_end(&mut decoded)
        .context("reading decoded artifact")?;
    Ok(decoded)
}

/// Returns an opener-ready decoded copy under `<cache_root>/decoded`, read under a storage
/// lease, preserving extensions and the run's relative asset layout; never a path under
/// `.corral`. A report's assets are its own task's artifacts in the same run directory.
pub fn artifact_path(ctx: &Ctx, a: &Artifact) -> Result<PathBuf> {
    let cache = decoded_root(ctx)?;
    if a.purged_at.is_some() {
        let path = cache.join(&a.path).with_extension("purged.txt");
        replace(&path, &stub(a))?;
        return Ok(path);
    }
    let parent = Path::new(&a.path).parent();
    let report = Path::new(&a.path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "html" | "htm" | "svg"));
    ctx.store.with_blob_lease(|| {
        let siblings: Vec<Artifact> = if report {
            let task = task_of(&a.path).context("artifact path names no task")?;
            ctx.store
                .artifact_rows(task, Some(&a.run))?
                .into_iter()
                .filter(|s| s.purged_at.is_none() && Path::new(&s.path).parent() == parent)
                .collect()
        } else {
            Vec::new()
        };
        if !siblings.iter().any(|s| s.source != a.source) {
            let path = cache.join(&a.path);
            replace(&path, &artifact_read(ctx, a)?)?;
            return Ok(path);
        }
        for sibling in &siblings {
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
    })
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
    let task = task_of(&artifact.path).context("artifact task not found")?;
    let mut result = None;
    ctx.store.update(task, |t| {
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

/// The stored `artifacts/...` path a reference names: the path itself, or the absolute path of
/// its legacy copy or of its decoded copy.
fn stored_path(ctx: &Ctx, reference: &str) -> Result<Option<String>> {
    let path = Path::new(reference);
    let relative = if path.is_absolute() {
        let decoded = decoded_root(ctx)?;
        path.strip_prefix(ctx.store.dir())
            .or_else(|_| path.strip_prefix(&decoded))
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    } else {
        Some(reference.to_string())
    };
    Ok(relative.filter(|p| task_of(p).is_some()))
}

/// The run segment of an `artifacts/<task>/<run>/<file>` path.
fn run_of(path: &str) -> Option<&str> {
    path.strip_prefix("artifacts/")?.split('/').nth(1)
}

/// Resolves a pin number, stable task:run:file reference, or exact stored/absolute snapshot path.
/// Purged artifacts remain resolvable even though their content file is gone. Only the named
/// task's artifact rows are read (one run's, unless a pin follows the latest), never the task.
pub fn resolve_artifact(ctx: &Ctx, reference: &str) -> Result<Artifact> {
    if let Some(path) = stored_path(ctx, reference)?
        && let (Some(task), Some(run)) = (task_of(&path), run_of(&path))
        && let Some(a) = ctx
            .store
            .artifact_rows(task, Some(run))?
            .into_iter()
            .find(|a| a.path == path)
    {
        return Ok(a);
    }
    let pins = ctx.store.pins()?;
    let pin = reference
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|n| pins.get(n));
    let reference = pin.map_or(reference, |p| p.target.as_str());
    let PinRef::Artifact { task, run, file } = PinRef::parse(reference)? else {
        bail!("expected an artifact reference (task:run:file)");
    };
    let task = Store::normalize_id(&task)?;
    ctx.store.head(&task).context("artifact task not found")?;
    let in_run = ctx.store.artifact_rows(&task, Some(&run))?;
    let exact = in_run
        .iter()
        .find(|a| corral_core::snapshot_name(a) == file);
    let a = exact
        .or_else(|| {
            in_run.iter().rev().find(|a| {
                let name = corral_core::snapshot_name(a);
                name.get(..8)
                    .is_some_and(|s| s.chars().all(|c| c.is_ascii_hexdigit()))
                    && name.get(8..9) == Some("-")
                    && name.get(9..) == Some(file.as_str())
            })
        })
        .context("artifact reference not found")?;
    if pin.is_some_and(|p| p.follow == Some(corral_core::Follow::Latest)) {
        // Artifacts append under the task lock; the newest same-title or same-source one wins.
        return Ok(ctx
            .store
            .artifact_rows(&task, None)?
            .into_iter()
            .rev()
            .find(|other| other.title == a.title || other.source == a.source)
            .unwrap_or_else(|| a.clone()));
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

/// Writes the legacy copy at the artifact's stored path (see the module comment).
pub(crate) fn mirror(ctx: &Ctx, a: &Artifact, bytes: &[u8]) -> Result<()> {
    replace(&ctx.store.dir().join(&a.path), bytes)
}
