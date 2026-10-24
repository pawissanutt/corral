//! No-follow directory handles for class-scoped deletion (plan T-253 section 8).
//!
//! A [`DirHandle`] opens one class root with `O_DIRECTORY|O_NOFOLLOW` and records its
//! `(dev, inode)` identity. Everything below the root is reached relative to that handle, one
//! plain component at a time and never through a symlink, and removed with `unlinkat`. Other
//! platforms refuse every operation rather than fall back to path-based deletion.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The `(dev, inode)` pair of a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirIdentity {
    pub dev: u64,
    pub ino: u64,
}

/// The plain components of a non-empty relative path. The empty path (the root itself), an
/// absolute path, `.` and `..` are refused.
pub fn plain_components(rel: &Path) -> Result<Vec<&OsStr>> {
    if rel.as_os_str().is_empty() {
        bail!("an empty entry path names the class root itself, which is never deleted");
    }
    rel.components()
        .map(|component| match component {
            Component::Normal(name) => Ok(name),
            other => bail!(
                "entry path {} refused: component {:?} is not a plain name",
                rel.display(),
                other.as_os_str()
            ),
        })
        .collect()
}

/// The identity of the real directory at `path`, looked up without following a final symlink.
pub fn lstat_identity(path: &Path) -> Result<DirIdentity> {
    let meta =
        std::fs::symlink_metadata(path).with_context(|| format!("lstat {}", path.display()))?;
    if meta.file_type().is_symlink() {
        bail!("{} is a symlink, not a real directory", path.display());
    }
    if !meta.is_dir() {
        bail!("{} is not a directory", path.display());
    }
    Ok(identity_of(&meta))
}

#[cfg(unix)]
fn identity_of(meta: &std::fs::Metadata) -> DirIdentity {
    use std::os::unix::fs::MetadataExt;
    DirIdentity {
        dev: meta.dev(),
        ino: meta.ino(),
    }
}

#[cfg(not(unix))]
fn identity_of(_meta: &std::fs::Metadata) -> DirIdentity {
    DirIdentity { dev: 0, ino: 0 }
}

/// One class root, opened without following symlinks.
#[derive(Debug)]
pub struct DirHandle {
    path: PathBuf,
    identity: DirIdentity,
    #[cfg(unix)]
    fd: std::os::fd::OwnedFd,
}

/// What [`DirHandle::remove_entry`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// The entry and its contents were deleted; `bytes` counts the regular files removed.
    Removed { bytes: u64 },
    /// The entry no longer existed (a previous attempt deleted it).
    Absent,
}

impl DirHandle {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn identity(&self) -> DirIdentity {
        self.identity
    }

    /// Fails unless `path` still names the directory this handle opened (same `dev`/`inode`, not
    /// a symlink).
    pub fn revalidate(&self) -> Result<()> {
        let now = lstat_identity(&self.path)
            .with_context(|| format!("revalidate class root {}", self.path.display()))?;
        if now != self.identity {
            bail!(
                "class root {} was replaced since it was opened (dev/inode {}:{} is now {}:{})",
                self.path.display(),
                self.identity.dev,
                self.identity.ino,
                now.dev,
                now.ino
            );
        }
        Ok(())
    }
}

#[cfg(unix)]
mod imp {
    use std::ffi::{CString, OsStr};
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use anyhow::{Context, Result, bail};

    use super::{DirHandle, Removal, identity_of, plain_components};

    const DIR_FLAGS: libc::c_int =
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

    fn c_name(name: &OsStr) -> io::Result<CString> {
        CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains NUL"))
    }

    fn check(ret: libc::c_int) -> io::Result<libc::c_int> {
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ret)
        }
    }

    fn open_at(dir: RawFd, name: &OsStr) -> io::Result<OwnedFd> {
        let name = c_name(name)?;
        // SAFETY: `name` is a valid NUL-terminated string; the returned fd is owned below.
        let fd = check(unsafe { libc::openat(dir, name.as_ptr(), DIR_FLAGS) })?;
        // SAFETY: `fd` is a fresh descriptor nothing else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// `Some((is_dir, regular-file size))` for the entry `name` in `dir`, without following a
    /// symlink.
    fn lstat_at(dir: RawFd, name: &OsStr) -> io::Result<Option<(bool, u64)>> {
        let c = c_name(name)?;
        // SAFETY: an all-zero `stat` is a valid value that fstatat overwrites.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: valid fd, NUL-terminated name and a writable `stat`.
        let ret = unsafe { libc::fstatat(dir, c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) };
        match check(ret) {
            Ok(_) => {
                let kind = st.st_mode & libc::S_IFMT;
                let size = if kind == libc::S_IFREG {
                    u64::try_from(st.st_size).unwrap_or(0)
                } else {
                    0
                };
                Ok(Some((kind == libc::S_IFDIR, size)))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn unlink_at(dir: RawFd, name: &OsStr, is_dir: bool) -> io::Result<()> {
        let c = c_name(name)?;
        let flags = if is_dir { libc::AT_REMOVEDIR } else { 0 };
        // SAFETY: valid fd and NUL-terminated name.
        check(unsafe { libc::unlinkat(dir, c.as_ptr(), flags) }).map(|_| ())
    }

    fn mkdir_at(dir: RawFd, name: &OsStr) -> io::Result<()> {
        let c = c_name(name)?;
        // SAFETY: valid fd and NUL-terminated name.
        check(unsafe { libc::mkdirat(dir, c.as_ptr(), 0o700) }).map(|_| ())
    }

    /// The names in the directory `dir`, read through the descriptor rather than a path.
    fn names(dir: &OwnedFd) -> io::Result<Vec<std::ffi::OsString>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(Path::new("/dev/fd").join(dir.as_raw_fd().to_string()))? {
            out.push(entry?.file_name());
        }
        Ok(out)
    }

    /// Deletes everything inside `dir` without following symlinks: symlinks and files are
    /// unlinked, subdirectories are entered with `O_NOFOLLOW` and removed after their contents.
    fn remove_contents(dir: &OwnedFd) -> io::Result<u64> {
        let mut bytes = 0;
        for name in names(dir)? {
            let Some((is_dir, size)) = lstat_at(dir.as_raw_fd(), &name)? else {
                continue;
            };
            if is_dir {
                let child = open_at(dir.as_raw_fd(), &name)?;
                bytes += remove_contents(&child)?;
            } else {
                bytes += size;
            }
            unlink_at(dir.as_raw_fd(), &name, is_dir)?;
        }
        Ok(bytes)
    }

    /// Walks `components` below `root` with `O_NOFOLLOW`, returning the parent's descriptor and
    /// the final name; `None` when an ancestor no longer exists.
    fn walk_parent<'a>(
        root: &OwnedFd,
        components: &[&'a OsStr],
    ) -> io::Result<Option<(OwnedFd, &'a OsStr)>> {
        let (last, ancestors) = components
            .split_last()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty entry path"))?;
        let mut dir = root.as_fd().try_clone_to_owned()?;
        for name in ancestors {
            dir = match open_at(dir.as_raw_fd(), name) {
                Ok(next) => next,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
        }
        Ok(Some((dir, last)))
    }

    impl DirHandle {
        /// Opens the real directory at `path` with `O_DIRECTORY|O_NOFOLLOW`.
        pub fn open(path: &Path) -> Result<Self> {
            let c = c_name(path.as_os_str())?;
            // SAFETY: valid NUL-terminated path; the returned fd is owned below.
            let fd = check(unsafe { libc::open(c.as_ptr(), DIR_FLAGS) }).with_context(|| {
                format!(
                    "open class root {} without following symlinks",
                    path.display()
                )
            })?;
            // SAFETY: `fd` is a fresh descriptor nothing else owns.
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            let meta = File::from(fd.try_clone()?)
                .metadata()
                .with_context(|| format!("fstat {}", path.display()))?;
            Ok(Self {
                path: path.to_path_buf(),
                identity: identity_of(&meta),
                fd,
            })
        }

        /// Creates the directory `rel` below the root (parents as needed, never through a
        /// symlink). The final component must not exist yet.
        pub fn create_entry(&self, rel: &Path) -> Result<()> {
            let components = plain_components(rel)?;
            let Some((last, ancestors)) = components.split_last() else {
                bail!("empty entry path");
            };
            let mut dir = self.fd.try_clone()?;
            for name in ancestors {
                match mkdir_at(dir.as_raw_fd(), name) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(error).with_context(|| format!("create {}", rel.display()));
                    }
                }
                dir = open_at(dir.as_raw_fd(), name).with_context(|| {
                    format!(
                        "enter {} below {} without following symlinks",
                        rel.display(),
                        self.path.display()
                    )
                })?;
            }
            mkdir_at(dir.as_raw_fd(), last)
                .with_context(|| format!("create {} below {}", rel.display(), self.path.display()))
        }

        /// Deletes the registered directory `rel` and its contents through this handle. Every
        /// component is opened with `O_NOFOLLOW`, so an ancestor or the entry itself being a
        /// symlink refuses. `confirm` runs and the root identity is re-validated immediately
        /// before deletion; an error from either refuses with nothing deleted.
        pub fn remove_entry(
            &self,
            rel: &Path,
            confirm: &dyn Fn() -> Result<()>,
        ) -> Result<Removal> {
            let components = plain_components(rel)?;
            self.revalidate()?;
            let refused = |what: &str| {
                format!(
                    "{what} {} below class root {}",
                    rel.display(),
                    self.path.display()
                )
            };
            let Some((parent, last)) =
                walk_parent(&self.fd, &components).with_context(|| refused("reach"))?
            else {
                return Ok(Removal::Absent);
            };
            let entry = match open_at(parent.as_raw_fd(), last) {
                Ok(entry) => entry,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok(Removal::Absent);
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        refused("refused (not a real directory, or a symlink) to open")
                    });
                }
            };
            confirm()?;
            self.revalidate()?;
            let bytes =
                remove_contents(&entry).with_context(|| refused("delete the contents of"))?;
            unlink_at(parent.as_raw_fd(), last, true).with_context(|| refused("remove"))?;
            Ok(Removal::Removed { bytes })
        }
    }
}

#[cfg(not(unix))]
impl DirHandle {
    pub fn open(path: &Path) -> Result<Self> {
        bail!(
            "class-scoped deletion needs no-follow directory handles, unsupported on this platform ({})",
            path.display()
        )
    }

    pub fn create_entry(&self, rel: &Path) -> Result<()> {
        bail!("unsupported on this platform: create {}", rel.display())
    }

    pub fn remove_entry(&self, rel: &Path, _confirm: &dyn Fn() -> Result<()>) -> Result<Removal> {
        bail!("unsupported on this platform: remove {}", rel.display())
    }
}
