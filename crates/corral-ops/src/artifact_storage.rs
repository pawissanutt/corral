//! Artifact bytes are content-addressed blobs (plan T-253 section 7): each owner holds one
//! `artifact-blobs` record per distinct content, whose document references the blob.
//! New publications never write legacy snapshot copies. Imported copies remain readable
//! until owner-run finalize; their encoding is described by artifact metadata.
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

/// Keyed artifact lookups, each a record written in the task update that writes its row:
/// `artifact-keys/<path>` names the row; `artifact-names/<task>/<run>/<file>` the newest
/// artifact whose snapshot is `<sha8>-<file>`; `artifact-latest/<task>/title|source/<value>`
/// the newest with that title or source. Artifacts attached without them (before keyed
/// lookups, or by other writers) fall back to their run's rows.
pub(crate) const ARTIFACT_KEYS: &str = "artifact-keys";
const ARTIFACT_NAMES: &str = "artifact-names";
const ARTIFACT_LATEST: &str = "artifact-latest";

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct KeyDoc {
    pub task: String,
    pub row: String,
    /// Changing with the row's retention state, so every row edit is a change of this record.
    pub tier: ArtifactTier,
    pub purged: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RowDoc {
    path: String,
    row: String,
}

/// The row key of the artifact at `index` of a task's artifacts.
fn row_key(index: usize) -> String {
    corral_core::store::codec::ordinal_key(index + 1)
}

/// The snapshot name without its `<sha8>-` prefix.
fn unprefixed(name: &str) -> Option<&str> {
    let hex = name.get(..8)?;
    (hex.chars().all(|c| c.is_ascii_hexdigit()) && name.get(8..9) == Some("-")).then(|| &name[9..])
}

/// Records the keyed lookups of the artifact at `index` of `task`, joining the caller's task
/// update. `added` also makes it the newest artifact of its name, title and source.
pub(crate) fn record_keys(
    ctx: &Ctx,
    task: &corral_core::Task,
    index: usize,
    added: bool,
) -> Result<()> {
    record_row_keys(ctx, &task.id, &task.artifacts[index], index, added)
}

/// [`record_keys`] of registration `index` (0-based over the task), given its artifact.
pub(crate) fn record_row_keys(
    ctx: &Ctx,
    task: &str,
    a: &Artifact,
    index: usize,
    added: bool,
) -> Result<()> {
    let row = row_key(index);
    let key = KeyDoc {
        task: task.to_owned(),
        row: row.clone(),
        tier: a.tier(),
        purged: a.purged_at.is_some(),
    };
    ctx.store.save_record(ARTIFACT_KEYS, &a.path, &key)?;
    if added {
        let to = RowDoc {
            path: a.path.clone(),
            row,
        };
        if let Some(file) = unprefixed(corral_core::snapshot_name(a)) {
            let name = format!("{task}/{}/{file}", a.run);
            ctx.store.save_record(ARTIFACT_NAMES, &name, &to)?;
        }
        let title = format!("{task}/title/{}", a.title);
        ctx.store.save_record(ARTIFACT_LATEST, &title, &to)?;
        let source = format!("{task}/source/{}", a.source);
        ctx.store.save_record(ARTIFACT_LATEST, &source, &to)?;
    }
    Ok(())
}

/// The artifact a lookup record points at, read as its one row.
fn pointed(ctx: &Ctx, task: &str, to: Option<RowDoc>) -> Result<Option<(String, Artifact)>> {
    let Some(to) = to else {
        return Ok(None);
    };
    Ok(ctx
        .store
        .artifact_row(task, &to.row)?
        .filter(|a| a.path == to.path)
        .map(|a| (to.row, a)))
}

/// The artifact stored at `path`, by its key record alone.
fn keyed(ctx: &Ctx, path: &str) -> Result<Option<Artifact>> {
    let Some(key) = ctx.store.record::<KeyDoc>(ARTIFACT_KEYS, path)? else {
        return Ok(None);
    };
    Ok(ctx
        .store
        .artifact_row(&key.task, &key.row)?
        .filter(|a| a.path == path))
}

/// The artifact stored at `path`: by its key record, else (an artifact attached before keyed
/// lookups) from its run's rows.
fn by_path(ctx: &Ctx, path: &str) -> Result<Option<Artifact>> {
    if let Some(owner) = task_of(path).filter(|owner| owner.starts_with("A-")) {
        return Ok(ctx
            .store
            .get_assistant(owner)?
            .artifacts
            .into_iter()
            .find(|artifact| artifact.path == path));
    }
    if let Some(a) = keyed(ctx, path)? {
        return Ok(Some(a));
    }
    let (Some(task), Some(run)) = (task_of(path), run_of(path)) else {
        return Ok(None);
    };
    Ok(ctx
        .store
        .artifact_rows(task, Some(run))?
        .into_iter()
        .find(|a| a.path == path))
}

/// The artifact `task:run:file` names: the exact snapshot, else the newest `<sha8>-<file>`, each
/// by its record; only artifacts attached before keyed lookups fall back to the run's rows.
fn by_reference(ctx: &Ctx, task: &str, run: &str, file: &str) -> Result<Option<Artifact>> {
    if task.starts_with("A-") {
        let session = ctx.store.get_assistant(task)?;
        let exact = session
            .artifacts
            .iter()
            .find(|artifact| corral_core::snapshot_name(artifact) == file);
        return Ok(exact
            .or_else(|| {
                session
                    .artifacts
                    .iter()
                    .rev()
                    .find(|artifact| unprefixed(corral_core::snapshot_name(artifact)) == Some(file))
            })
            .cloned());
    }
    if let Some(a) = keyed(ctx, &format!("artifacts/{task}/{run}/{file}"))? {
        return Ok(Some(a));
    }
    let to = ctx
        .store
        .record::<RowDoc>(ARTIFACT_NAMES, &format!("{task}/{run}/{file}"))?;
    if let Some((_, a)) = pointed(ctx, task, to)? {
        return Ok(Some(a));
    }
    let rows = ctx.store.artifact_rows(task, Some(run))?;
    let exact = rows.iter().find(|a| corral_core::snapshot_name(a) == file);
    Ok(exact
        .or_else(|| {
            rows.iter()
                .rev()
                .find(|a| unprefixed(corral_core::snapshot_name(a)) == Some(file))
        })
        .cloned())
}

/// The newest artifact of `task` sharing `anchor`'s title or source (a followed pin).
fn newest_like(ctx: &Ctx, task: &str, anchor: &Artifact) -> Result<Artifact> {
    if task.starts_with("A-") {
        return Ok(ctx
            .store
            .get_assistant(task)?
            .artifacts
            .into_iter()
            .rev()
            .find(|artifact| artifact.title == anchor.title || artifact.source == anchor.source)
            .unwrap_or_else(|| anchor.clone()));
    }
    let lookup = |kind: &str, value: &str| {
        ctx.store
            .record::<RowDoc>(ARTIFACT_LATEST, &format!("{task}/{kind}/{value}"))
    };
    let title = pointed(ctx, task, lookup("title", &anchor.title)?)?;
    let source = pointed(ctx, task, lookup("source", &anchor.source)?)?;
    if let Some((_, newest)) = title.into_iter().chain(source).max_by(|a, b| a.0.cmp(&b.0)) {
        return Ok(newest);
    }
    Ok(ctx
        .store
        .artifact_rows(task, None)?
        .into_iter()
        .rev()
        .find(|other| other.title == anchor.title || other.source == anchor.source)
        .unwrap_or_else(|| anchor.clone()))
}

/// The task an artifact belongs to, from its `artifacts/<task>/<run>/<file>` path.
pub(crate) fn task_of(path: &str) -> Option<&str> {
    let relative = path.strip_prefix("artifacts/")?;
    let mut parts = relative.split('/');
    let first = parts.next()?;
    if first == "assistant" {
        parts.next().filter(|owner| owner.starts_with("A-"))
    } else {
        (!first.is_empty()).then_some(first)
    }
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

thread_local! {
    static HELD: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Artifact-storage access. Nested acquisitions on the holding thread reuse it, so a caller
/// can hold storage across work that attaches artifacts (T-505).
pub(crate) struct StorageGuard {
    _file: Option<fs::File>,
}

impl StorageGuard {
    fn enter(file: Option<fs::File>) -> Self {
        HELD.with(|held| held.set(held.get() + 1));
        Self { _file: file }
    }
}

impl Drop for StorageGuard {
    fn drop(&mut self) {
        HELD.with(|held| held.set(held.get().saturating_sub(1)));
    }
}

pub(crate) fn held_here() -> bool {
    HELD.with(|held| held.get() > 0)
}

fn open_lock(ctx: &Ctx) -> Result<fs::File> {
    let dir = ctx.store.dir().join("lock");
    fs::create_dir_all(&dir).context("creating artifact lock directory")?;
    fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("artifact-storage.lock"))
        .context("opening artifact storage lock")
}

/// Shared access for artifact writers and readers. A writer only adds new snapshots, each
/// atomically before its task's metadata names it, and the task lock serializes that metadata,
/// so writers never wait for one another: a done or verdict cannot starve behind hooks (T-659).
pub(crate) fn storage_lock(ctx: &Ctx) -> Result<StorageGuard> {
    if held_here() {
        return Ok(StorageGuard::enter(None));
    }
    let file = open_lock(ctx)?;
    crate::agent::effects::failpoint("artifact_storage_wait");
    file.lock_shared().context("locking artifact storage")?;
    crate::agent::effects::failpoint("artifact_storage_held");
    Ok(StorageGuard::enter(Some(file)))
}

/// Exclusive access for GC, the only operation that rewrites or removes existing snapshots.
/// Lock order with the search index (T-659): GC takes this holding nothing and releases it
/// before refreshing the index, and nothing awaits the index lock while holding storage, so
/// an index owner waiting here for shared storage never closes a cycle.
pub(crate) fn storage_exclusive(ctx: &Ctx) -> Result<StorageGuard> {
    if held_here() {
        bail!("artifact storage is already held on this thread");
    }
    let file = open_lock(ctx)?;
    file.lock().context("locking artifact storage")?;
    crate::agent::effects::failpoint("artifact_storage_exclusive");
    Ok(StorageGuard::enter(Some(file)))
}

/// Waits until no exclusive holder (GC) has artifact storage, without keeping access: a pass
/// calls it before taking a lease its artifact writes must not park (T-505).
pub(crate) fn await_storage(ctx: &Ctx) -> Result<()> {
    if held_here() {
        return Ok(());
    }
    let file = open_lock(ctx)?;
    crate::agent::effects::failpoint("artifact_storage_wait");
    file.lock_shared().context("waiting for artifact storage")?;
    file.unlock().context("releasing artifact storage")
}

/// None while GC has artifact storage.
pub(crate) fn try_storage_lock(ctx: &Ctx) -> Result<Option<StorageGuard>> {
    if held_here() {
        return Ok(Some(StorageGuard::enter(None)));
    }
    let file = open_lock(ctx)?;
    match file.try_lock_shared() {
        Ok(()) => Ok(Some(StorageGuard::enter(Some(file)))),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(error)) => Err(error).context("locking artifact storage"),
    }
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
    artifact_read_confined_limited(ctx, a, u64::MAX)
}

/// The same confined/verified read, capped at `max + 1` decoded bytes for evidence validation.
pub(crate) fn artifact_read_confined_limited(ctx: &Ctx, a: &Artifact, max: u64) -> Result<Vec<u8>> {
    if a.purged_at.is_some() {
        return Ok(stub(a));
    }
    let file = match open_beneath_artifacts(ctx, &a.path) {
        Ok(file) => file,
        Err(error) if absent(&error) => {
            let Some(reader) = blob_reader(ctx, a)? else {
                return Err(error);
            };
            let mut decoded = Vec::new();
            reader
                .take(max.saturating_add(1))
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
    let reader: Box<dyn Read> = if a.compressed_bytes.is_some() {
        Box::new(MultiGzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut decoded = Vec::new();
    reader
        .take(max.saturating_add(1))
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
    let root = snapshot_root(ctx.store.dir());
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
    let root = fs::canonicalize(snapshot_root(ctx.store.dir())).context("resolving artifacts")?;
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
            let rows = if task.starts_with("A-") {
                ctx.store.get_assistant(task)?.artifacts
            } else {
                ctx.store.artifact_rows(task, Some(&a.run))?
            };
            rows.into_iter()
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
    crate::assistant_artifacts::check_owner(Some(task))?;
    if task.starts_with("A-") {
        return crate::assistant_artifacts::update(ctx, task, &artifact.path, |artifact| {
            artifact.tier_override = Some(tier)
        });
    }
    let mut result = None;
    ctx.store.update(task, |t| {
        let index = t
            .artifacts
            .iter()
            .rposition(|a| a.path == artifact.path)
            .context("artifact reference not found")?;
        t.artifacts[index].tier_override = Some(tier);
        result = Some(t.artifacts[index].clone());
        record_keys(ctx, t, index, false)
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
/// Purged artifacts remain resolvable even though their content file is gone. One artifact is
/// read by its key record as one row; only artifacts attached without keyed lookups fall back
/// to their run's (or, for a followed pin, their task's) artifact rows.
pub fn resolve_artifact(ctx: &Ctx, reference: &str) -> Result<Artifact> {
    if let Some(path) = stored_path(ctx, reference)?
        && let Some(a) = by_path(ctx, &path)?
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
    let task = if task.starts_with("A-") {
        corral_core::assistant::validate_id(&task)?;
        task
    } else {
        Store::normalize_id(&task)?
    };
    let a = match by_reference(ctx, &task, &run, &file)? {
        Some(a) => a,
        None => {
            if task.starts_with("A-") {
                ctx.store
                    .get_assistant(&task)
                    .context("artifact session not found")?;
            } else {
                ctx.store.head(&task).context("artifact task not found")?;
            }
            bail!("artifact reference not found");
        }
    };
    if pin.is_some_and(|p| p.follow == Some(corral_core::Follow::Latest)) {
        return newest_like(ctx, &task, &a);
    }
    Ok(a)
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
    for session in ctx.store.list_assistants()? {
        paths.extend(
            session
                .artifacts
                .into_iter()
                .filter(|artifact| {
                    artifact.tier() == ArtifactTier::Keep
                        || artifact.kind == corral_core::ArtifactKind::Screenshot
                })
                .map(|artifact| artifact.path),
        );
    }
    for pin in ctx.store.pins()? {
        // Both actual owner kinds protect their anchor AND current follow-latest target.
        // The keyed resolver checks canonical blobs, not just legacy snapshot paths.
        let anchor = corral_core::Pin {
            follow: None,
            ..pin.clone()
        };
        for selection in [&pin, &anchor] {
            if let Ok(PinTarget::Artifact { artifact, .. }) = ctx.store.resolve_pin_head(selection)
            {
                paths.insert(artifact.path);
            }
        }
    }
    Ok(paths)
}

/// Publishes session artifact bytes through the same canonical blob store as task evidence.
pub(crate) fn snapshot(ctx: &Ctx, bytes: &[u8], artifact: &Artifact) -> Result<Option<u64>> {
    let owner = task_of(&artifact.path).context("artifact owner missing")?;
    store_blob(ctx, owner, &artifact.sha256, bytes)?;
    Ok(None)
}

/// The imported legacy snapshot root, retained in place until `migrate --finalize`.
pub(crate) fn snapshot_root(store_dir: &Path) -> PathBuf {
    store_dir.join("artifacts") // path-guard: allow (legacy snapshot mirror until finalize)
}
