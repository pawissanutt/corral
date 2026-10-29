//! Durable filesystem steps: every write is tmp -> fsync -> rename -> fsync of the directory,
//! every rename and removal is followed by an fsync of the directories it changed.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};

pub(crate) fn sync_dir(dir: &Path) -> Result<()> {
    fs::File::open(dir)
        .and_then(|file| file.sync_all())
        .with_context(|| format!("syncing {}", dir.display()))
}

pub(crate) fn sync_file(path: &Path) -> Result<()> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .with_context(|| format!("syncing {}", path.display()))
}

/// Writes `bytes` to `path` durably; `between` runs after the synced tmp file exists and
/// before the rename (the B14 window of the fence).
pub(crate) fn write(path: &Path, bytes: &[u8], between: impl FnOnce() -> Result<()>) -> Result<()> {
    let dir = path.parent().context("durable write path has no parent")?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    let mut file = fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("writing {}", tmp.display()))?;
    drop(file);
    between()?;
    fs::rename(&tmp, path).with_context(|| format!("publishing {}", path.display()))?;
    sync_dir(dir)
}

/// Removes `path` (a file) durably; an absent file is already removed.
pub(crate) fn unlink(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("removing {}", path.display())),
    }
    sync_dir(path.parent().context("removed path has no parent")?)
}

/// Renames a directory as one step and syncs both parents.
pub(crate) fn rename_dir(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to)
        .with_context(|| format!("renaming {} to {}", from.display(), to.display()))?;
    let to_parent = to.parent().context("rename target has no parent")?;
    sync_dir(to_parent)?;
    let from_parent = from.parent().context("rename source has no parent")?;
    if from_parent != to_parent {
        sync_dir(from_parent)?;
    }
    Ok(())
}

/// What `lstat` sees at a migration path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Seen {
    Absent,
    Dir,
    Symlink,
    Other,
}

impl Seen {
    pub fn present(self) -> bool {
        self != Seen::Absent
    }
}

pub(crate) fn seen(path: &Path) -> Result<Seen> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Ok(Seen::Symlink),
        Ok(meta) if meta.is_dir() => Ok(Seen::Dir),
        Ok(_) => Ok(Seen::Other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Seen::Absent),
        Err(error) => Err(error).with_context(|| format!("inspecting {}", path.display())),
    }
}

/// Deletes the directory tree at `path` without following symlinks, idempotently; `after_first`
/// runs once the first entry is gone (the "mid-deletion" barriers). Syncs the parent at the end.
pub(crate) fn remove_tree(path: &Path, after_first: impl FnOnce() -> Result<()>) -> Result<()> {
    let mut after_first = Some(after_first);
    match seen(path)? {
        Seen::Absent => return Ok(()),
        Seen::Dir => {}
        other => bail!("refusing to delete {} ({other:?})", path.display()),
    }
    remove_children(path, &mut after_first)?;
    match fs::remove_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("removing {}", path.display())),
    }
    sync_dir(path.parent().context("removed tree has no parent")?)
}

type AfterFirst<F> = Option<F>;

fn remove_children<F: FnOnce() -> Result<()>>(dir: &Path, after: &mut AfterFirst<F>) -> Result<()> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<io::Result<Vec<_>>>()
        .with_context(|| format!("reading {}", dir.display()))?;
    entries.sort();
    for entry in entries {
        let meta = match fs::symlink_metadata(&entry) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", entry.display()));
            }
        };
        if meta.is_dir() {
            remove_children(&entry, after)?;
            fs::remove_dir(&entry).with_context(|| format!("removing {}", entry.display()))?;
        } else {
            fs::remove_file(&entry).with_context(|| format!("removing {}", entry.display()))?;
        }
        if let Some(first) = after.take() {
            sync_dir(dir)?;
            first()?;
        }
    }
    Ok(())
}
