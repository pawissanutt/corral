//! No-follow directory handles for class-scoped deletion (plan T-253 section 8).
//!
//! A [`DirHandle`] opens one class root by walking its absolute path from `/` one component at
//! a time with `O_DIRECTORY|O_NOFOLLOW`, so no component of the root, ancestors included, may
//! be a symlink, and records the root's `(dev, inode)`. Everything below the root is reached
//! relative to that handle the same way and removed with `unlinkat`. Immediately before a
//! deletion the root walk is repeated and every directory between the root and the entry is
//! re-opened from the root handle and compared by identity with the descriptors in use, so a
//! swapped root, ancestor or entry refuses with nothing deleted. Other platforms refuse every
//! operation rather than fall back to path-based deletion.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};

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
}

#[cfg(unix)]
mod imp {
    use std::ffi::{CString, OsStr};
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Component, Path};

    use anyhow::{Context, Result, bail};

    use super::{DirHandle, DirIdentity, Removal, plain_components};

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

    fn owned(fd: libc::c_int) -> OwnedFd {
        // SAFETY: `fd` is a fresh descriptor returned by a successful open; nothing else owns it.
        unsafe { OwnedFd::from_raw_fd(fd) }
    }

    fn open_at(dir: RawFd, name: &OsStr) -> io::Result<OwnedFd> {
        let name = c_name(name)?;
        // SAFETY: valid fd and NUL-terminated name.
        check(unsafe { libc::openat(dir, name.as_ptr(), DIR_FLAGS) }).map(owned)
    }

    fn identity(fd: &OwnedFd) -> io::Result<DirIdentity> {
        let meta = File::from(fd.try_clone()?).metadata()?;
        Ok(DirIdentity {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }

    /// Opens the absolute `path` from `/`, every component with `O_NOFOLLOW`; with `create`,
    /// missing components are made with `mkdirat` on the way.
    fn open_absolute(path: &Path, create: bool) -> io::Result<OwnedFd> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "class root is not absolute",
            ));
        }
        let root = c_name(OsStr::new("/"))?;
        // SAFETY: valid NUL-terminated path.
        let mut dir = check(unsafe { libc::open(root.as_ptr(), DIR_FLAGS) }).map(owned)?;
        for component in path.components() {
            let name = match component {
                Component::RootDir => continue,
                Component::Normal(name) => name,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "class root has a non-plain component",
                    ));
                }
            };
            dir = match open_at(dir.as_raw_fd(), name) {
                Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                    match mkdir_at(dir.as_raw_fd(), name) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error),
                    }
                    open_at(dir.as_raw_fd(), name)?
                }
                other => other?,
            };
        }
        Ok(dir)
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

    /// Creates the absolute directory `path` and its missing parents, never through a symlink.
    pub fn create_dir_all_nofollow(path: &Path) -> Result<()> {
        open_absolute(path, true)
            .map(drop)
            .with_context(|| format!("create {} without following symlinks", path.display()))
    }

    impl DirHandle {
        /// Opens the real directory at the absolute `path`, refusing a symlink in any component.
        pub fn open(path: &Path) -> Result<Self> {
            let fd = open_absolute(path, false).with_context(|| {
                format!(
                    "open class root {} without following symlinks in any component",
                    path.display()
                )
            })?;
            Ok(Self {
                path: path.to_path_buf(),
                identity: identity(&fd)?,
                fd,
            })
        }

        /// Fails unless walking `path` from `/` without following symlinks still reaches the
        /// directory this handle opened (same `dev`/`inode`).
        pub fn revalidate(&self) -> Result<()> {
            let fd = open_absolute(&self.path, false).with_context(|| {
                format!(
                    "class root {} was replaced since it was opened (it or an ancestor is gone or a symlink)",
                    self.path.display()
                )
            })?;
            let now = identity(&fd)?;
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

        /// Creates the directory `rel` below the root (parents as needed, never through a
        /// symlink). The final component must not exist yet.
        pub fn create_entry(&self, rel: &Path) -> Result<()> {
            let components = plain_components(rel)?;
            let Some((last, ancestors)) = components.split_last() else {
                bail!("empty entry path");
            };
            self.revalidate()?;
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

        /// Re-opens every component of `rel` from the root handle and requires each to be the
        /// directory in `held` (ancestors, then the entry).
        fn verify_chain(&self, components: &[&OsStr], held: &[OwnedFd]) -> Result<()> {
            let mut dir = self.fd.try_clone()?;
            for (name, expected) in components.iter().zip(held) {
                let next = open_at(dir.as_raw_fd(), name).with_context(|| {
                    format!(
                        "{name:?} below {} was moved or replaced",
                        self.path.display()
                    )
                })?;
                if identity(&next)? != identity(expected)? {
                    bail!(
                        "{name:?} below {} was replaced since it was opened",
                        self.path.display()
                    );
                }
                dir = next;
            }
            Ok(())
        }

        /// Deletes the registered directory `rel` and its contents through this handle. Every
        /// component is opened with `O_NOFOLLOW`, so an ancestor or the entry itself being a
        /// symlink refuses. Immediately before deletion, `confirm` runs, the root walk is
        /// repeated and every component is re-opened from the root and compared by identity;
        /// any failure refuses with nothing deleted.
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
            let mut held: Vec<OwnedFd> = Vec::with_capacity(components.len());
            for name in &components {
                let dir = held.last().unwrap_or(&self.fd);
                match open_at(dir.as_raw_fd(), name) {
                    Ok(next) => held.push(next),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        return Ok(Removal::Absent);
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            refused("refused (not a real directory, or a symlink) to open")
                        });
                    }
                }
            }
            confirm()?;
            self.revalidate()?;
            self.verify_chain(&components, &held)
                .with_context(|| refused("refused to delete"))?;
            let (entry, ancestors) = held.split_last().context("empty entry path")?;
            let parent = ancestors.last().unwrap_or(&self.fd);
            let last = components.last().context("empty entry path")?;
            let bytes =
                remove_contents(entry).with_context(|| refused("delete the contents of"))?;
            unlink_at(parent.as_raw_fd(), last, true).with_context(|| refused("remove"))?;
            Ok(Removal::Removed { bytes })
        }
    }
}

#[cfg(unix)]
pub use imp::create_dir_all_nofollow;

#[cfg(not(unix))]
pub fn create_dir_all_nofollow(path: &Path) -> Result<()> {
    bail!(
        "no-follow directory creation is unsupported on this platform ({})",
        path.display()
    )
}

#[cfg(not(unix))]
impl DirHandle {
    pub fn open(path: &Path) -> Result<Self> {
        bail!(
            "class-scoped deletion needs no-follow directory handles, unsupported on this platform ({})",
            path.display()
        )
    }

    pub fn revalidate(&self) -> Result<()> {
        bail!(
            "unsupported on this platform: revalidate {}",
            self.path.display()
        )
    }

    pub fn create_entry(&self, rel: &Path) -> Result<()> {
        bail!("unsupported on this platform: create {}", rel.display())
    }

    pub fn remove_entry(&self, rel: &Path, _confirm: &dyn Fn() -> Result<()>) -> Result<Removal> {
        bail!("unsupported on this platform: remove {}", rel.display())
    }
}
