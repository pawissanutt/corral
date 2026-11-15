//! Transactional native record store, shared by every app instance for this OS user.
//!
//! `read()` locks and returns one consistent `(revision, records)` snapshot; an absent
//! `durable.json` is revision zero with no records. `compare_exchange(expected, patch)`
//! locks, checks the revision, and applies only the named records (`None` deletes).
//! `Ok(false)` means a revision conflict with no changes. `Ok(true)` means the next
//! revision and its entire patched aggregate were file-synced, atomically renamed,
//! and directory-synced. Even an empty patch increments the revision.
//!
//! Errors are never acknowledgements. In particular, a directory-sync error after
//! rename can leave the new revision visible: callers must reread/reconcile rather
//! than infer rollback. Retry pure record changes against a fresh snapshot, never
//! replace it with a stale whole-outbox value. Poll `read()`'s committed revision to
//! observe other instances; no separate, potentially inconsistent revision file exists.
//!
//! Files live at `$XDG_STATE_HOME/corral-app` (fallback `$HOME/.local/state`). The
//! directory is private 0700; `durable.json` and the persistent `durable.lock` are
//! private 0600 files. Never unlink the lock: all contenders must lock the same inode.
//! The encoded aggregate, including revision and envelope, is limited to 8 MiB.
use serde_json::Value;
use std::collections::BTreeMap;

/// Read one transactionally consistent snapshot, or an attributed storage failure.
#[cfg(not(test))]
pub(super) async fn read() -> Result<(u64, BTreeMap<String, Value>), String> {
    #[cfg(unix)]
    {
        nsh_core::prelude::blocking(|| read_in(&state_root()?))
            .await
            .map_err(|error| format!("native durable store worker closed: {error}"))?
    }
    #[cfg(not(unix))]
    {
        Err(
            "native durable store: private transactional storage is unavailable on this platform"
                .into(),
        )
    }
}

#[cfg(not(test))]
/// Apply a per-record patch only at `expected`; true is a durable commit acknowledgement.
pub(super) async fn compare_exchange(
    expected: u64,
    changes: BTreeMap<String, Option<Value>>,
) -> Result<bool, String> {
    #[cfg(unix)]
    {
        nsh_core::prelude::blocking(move || compare_exchange_in(&state_root()?, expected, changes))
            .await
            .map_err(|error| format!("native durable commit worker closed: {error}"))?
    }
    #[cfg(not(unix))]
    {
        let _ = (expected, changes);
        Err(
            "native durable store: private transactional storage is unavailable on this platform"
                .into(),
        )
    }
}

#[cfg(unix)]
use {
    rustix::fs::{AtFlags, Mode, OFlags},
    serde::{Deserialize, Serialize},
    std::{
        fs,
        io::{self, Read, Write},
        os::unix::fs::MetadataExt,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    },
};

#[cfg(unix)]
const MAX_BYTES: usize = 8 * 1024 * 1024;
#[cfg(unix)]
const PRIVATE_FILE: Mode = Mode::RUSR.union(Mode::WUSR);
#[cfg(unix)]
static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    revision: u64,
    records: BTreeMap<String, Value>,
}

#[cfg(unix)]
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            records: BTreeMap::new(),
        }
    }
}

#[cfg(unix)]
fn failure(operation: &str, error: impl std::fmt::Display) -> String {
    format!("native durable store: {operation}: {error}")
}

#[cfg(unix)]
#[cfg(not(test))]
fn state_root() -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        let root = PathBuf::from(root);
        if root.is_absolute() {
            return Ok(root);
        }
    }
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".local/state"))
        .ok_or_else(|| failure("cannot locate durable.json", "set XDG_STATE_HOME or HOME"))
}

#[cfg(unix)]
struct Locked {
    // Keep the parent open so a first commit also persists the corral-app directory entry.
    parent: fs::File,
    directory: fs::File,
    _lock: fs::File,
}

#[cfg(unix)]
fn check_private(metadata: &fs::Metadata, name: &str, directory: bool) -> Result<(), String> {
    let valid_kind = if directory {
        metadata.is_dir()
    } else {
        metadata.is_file()
    };
    let mode = if directory { 0o700 } else { 0o600 };
    if !valid_kind
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.mode() & 0o777 != mode
        || (!directory && metadata.nlink() != 1)
    {
        return Err(failure(
            name,
            if directory {
                "must be a private 0700 directory owned by you, not a symlink"
            } else {
                "must be a private 0600 regular file owned by you, without links"
            },
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn lock(root: &Path) -> Result<Locked, String> {
    // Persist newly created state ancestors too, not just the final JSON directory.
    let mut missing = Vec::new();
    let mut ancestor = root;
    while !ancestor.exists() {
        missing.push(ancestor.to_path_buf());
        ancestor = ancestor
            .parent()
            .ok_or_else(|| failure("state directory", "has no existing ancestor"))?;
    }
    fs::create_dir_all(root).map_err(|error| failure("cannot create state directory", error))?;
    for path in missing.iter().rev() {
        if let Some(parent) = path.parent() {
            fs::File::open(parent)
                .and_then(|file| file.sync_all())
                .map_err(|error| failure("cannot sync state directory parent", error))?;
        }
    }
    let parent: fs::File = rustix::fs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| failure("cannot open state directory", error))?
    .into();
    match rustix::fs::mkdirat(&parent, "corral-app", Mode::RWXU) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(error) => return Err(failure("cannot create corral-app directory", error)),
    }
    let directory: fs::File = rustix::fs::openat(
        &parent,
        "corral-app",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| failure("cannot open private corral-app directory", error))?
    .into();
    check_private(
        &directory
            .metadata()
            .map_err(|error| failure("cannot inspect corral-app", error))?,
        "corral-app",
        true,
    )?;
    let file: fs::File = rustix::fs::openat(
        &directory,
        "durable.lock",
        OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        PRIVATE_FILE,
    )
    .map_err(|error| failure("cannot open durable.lock", error))?
    .into();
    check_private(
        &file
            .metadata()
            .map_err(|error| failure("cannot inspect durable.lock", error))?,
        "durable.lock",
        false,
    )?;
    file.lock()
        .map_err(|error| failure("cannot lock durable.lock", error))?;
    Ok(Locked {
        parent,
        directory,
        _lock: file,
    })
}

#[cfg(unix)]
fn read_locked(locked: &Locked) -> Result<Snapshot, String> {
    let file: fs::File = match rustix::fs::openat(
        &locked.directory,
        "durable.json",
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => file.into(),
        Err(rustix::io::Errno::NOENT) => return Ok(Snapshot::default()),
        Err(error) => return Err(failure("cannot open durable.json", error)),
    };
    let metadata = file
        .metadata()
        .map_err(|error| failure("cannot inspect durable.json", error))?;
    check_private(&metadata, "durable.json", false)?;
    if metadata.len() > MAX_BYTES as u64 {
        return Err(failure(
            "cannot read durable.json",
            "encoded aggregate exceeds 8 MiB (8388608 bytes)",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| failure("cannot read durable.json", error))?;
    if bytes.len() > MAX_BYTES {
        return Err(failure(
            "cannot read durable.json",
            "encoded aggregate exceeds 8 MiB (8388608 bytes)",
        ));
    }
    let saved: Snapshot = serde_json::from_slice(&bytes)
        .map_err(|error| failure("cannot decode durable.json", error))?;
    if saved.version != 1 {
        return Err(failure("cannot decode durable.json", "unsupported version"));
    }
    Ok(saved)
}

#[cfg(unix)]
fn read_in(root: &Path) -> Result<(u64, BTreeMap<String, Value>), String> {
    let locked = lock(root)?;
    let saved = read_locked(&locked)?;
    Ok((saved.revision, saved.records))
}

#[cfg(unix)]
struct BoundedEncoding(Vec<u8>);

#[cfg(unix)]
impl Write for BoundedEncoding {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_BYTES - self.0.len() {
            return Err(io::Error::other(
                "encoded aggregate exceeds 8 MiB (8388608 bytes)",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
fn persist(locked: &Locked, saved: &Snapshot) -> Result<(), String> {
    let mut encoded = BoundedEncoding(Vec::new());
    serde_json::to_writer(&mut encoded, saved)
        .map_err(|error| failure("cannot encode durable.json", error))?;
    let stage = format!(
        "durable.json.{}.{}.tmp",
        std::process::id(),
        NEXT_STAGE.fetch_add(1, Ordering::Relaxed),
    );
    let mut file: fs::File = rustix::fs::openat(
        &locked.directory,
        stage.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        PRIVATE_FILE,
    )
    .map_err(|error| failure("cannot stage durable.json", error))?
    .into();
    let result = (|| {
        check_private(
            &file
                .metadata()
                .map_err(|error| failure("cannot inspect staged durable.json", error))?,
            "staged durable.json",
            false,
        )?;
        #[cfg(test)]
        inject_failure(Fault::Write)?;
        file.write_all(&encoded.0)
            .map_err(|error| failure("cannot write durable.json", error))?;
        #[cfg(test)]
        inject_failure(Fault::FileSync)?;
        file.sync_all()
            .map_err(|error| failure("cannot sync durable.json", error))?;
        #[cfg(test)]
        inject_failure(Fault::Rename)?;
        rustix::fs::renameat(
            &locked.directory,
            stage.as_str(),
            &locked.directory,
            "durable.json",
        )
        .map_err(|error| failure("cannot replace durable.json", error))?;
        #[cfg(test)]
        inject_failure(Fault::DirectorySync)?;
        locked
            .directory
            .sync_all()
            .map_err(|error| failure("cannot sync corral-app directory", error))?;
        locked
            .parent
            .sync_all()
            .map_err(|error| failure("cannot sync corral-app parent directory", error))?;
        Ok(())
    })();
    if result.is_err() {
        // The stage may already have been renamed. Never remove the committed file.
        let _ = rustix::fs::unlinkat(&locked.directory, stage.as_str(), AtFlags::empty());
    }
    result
}

#[cfg(unix)]
fn compare_exchange_in(
    root: &Path,
    expected: u64,
    changes: BTreeMap<String, Option<Value>>,
) -> Result<bool, String> {
    let locked = lock(root)?;
    let mut saved = read_locked(&locked)?;
    if saved.revision != expected {
        return Ok(false);
    }
    saved.revision = saved
        .revision
        .checked_add(1)
        .ok_or_else(|| failure("cannot update durable.json", "revision exhausted"))?;
    for (key, value) in changes {
        match value {
            Some(value) => {
                saved.records.insert(key, value);
            }
            None => {
                saved.records.remove(&key);
            }
        }
    }
    persist(&locked, &saved)?;
    Ok(true)
}

#[cfg(all(test, unix))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    Write,
    FileSync,
    Rename,
    DirectorySync,
}

#[cfg(all(test, unix))]
std::thread_local! {
    static FAILURE: std::cell::Cell<Option<Fault>> = const { std::cell::Cell::new(None) };
}

#[cfg(all(test, unix))]
fn inject_failure(point: Fault) -> Result<(), String> {
    if FAILURE.with(|failure| failure.get() == Some(point)) {
        Err(failure(
            "injected durable.json I/O refusal",
            format_args!("{point:?}"),
        ))
    } else {
        Ok(())
    }
}

#[cfg(all(test, unix))]
#[path = "durable_native_tests.rs"]
mod tests;
