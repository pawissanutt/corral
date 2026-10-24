//! Filesystem admission (plan T-253 section 1, requirement 7; risk 6): SQLite's WAL and corral's
//! `flock` leases need a local filesystem, so a database on 9p, drvfs, NFS, SMB or FUSE is
//! refused before any file is created.

use std::path::Path;

use anyhow::{Result, bail};

/// Reports the filesystem type name of an existing path (`ext4`, `9p`, `nfs`, ...).
pub type FilesystemProbe = fn(&Path) -> Result<String>;

const REFUSED: [&str; 16] = [
    "9p", "v9fs", "drvfs", "nfs", "nfs4", "smb", "smb2", "smb3", "smbfs", "cifs", "fuse",
    "fuseblk", "macfuse", "osxfuse", "afpfs", "webdav",
];

pub fn is_refused_filesystem(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    REFUSED.contains(&name.as_str()) || name.starts_with("fuse.")
}

/// Refuses `path` when its nearest existing ancestor lives on a refused filesystem. A relative
/// path with no existing named ancestor resolves against the working directory, `.`.
pub(super) fn ensure_local(path: &Path, probe: FilesystemProbe) -> Result<()> {
    let filesystem = probe(nearest_existing(path))?;
    if is_refused_filesystem(&filesystem) {
        bail!(
            "refusing to open the corral database at {}: {} is on a {filesystem} filesystem; \
             the workspace must live on a local filesystem",
            path.display(),
            nearest_existing(path).display()
        );
    }
    Ok(())
}

fn nearest_existing(path: &Path) -> &Path {
    let mut existing = path;
    loop {
        if existing.as_os_str().is_empty() {
            return Path::new(".");
        }
        if existing.exists() {
            return existing;
        }
        match existing.parent() {
            Some(parent) => existing = parent,
            None => return existing,
        }
    }
}

#[cfg(target_os = "linux")]
pub fn statfs_probe(path: &Path) -> Result<String> {
    use std::os::unix::ffi::OsStrExt;

    use anyhow::Context;

    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("path {} contains a NUL byte", path.display()))?;
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a NUL-terminated string and `stat` is a writable statfs buffer.
    let status = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) };
    if status != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("statfs {}", path.display()));
    }
    #[allow(clippy::unnecessary_cast)]
    let magic = stat.f_type as u64;
    Ok(match magic {
        0x0102_1997 => "9p".to_owned(),
        0x5346_4846 => "drvfs".to_owned(),
        0x6969 => "nfs".to_owned(),
        0x517b => "smb".to_owned(),
        0xfe53_4d42 => "smb2".to_owned(),
        0xff53_4d42 => "cifs".to_owned(),
        0x6573_5546 => "fuse".to_owned(),
        0xef53 => "ext4".to_owned(),
        0x0102_1994 => "tmpfs".to_owned(),
        0x9123_683e => "btrfs".to_owned(),
        0x5846_5342 => "xfs".to_owned(),
        0x2fc1_2fc1 => "zfs".to_owned(),
        0x794c_7630 => "overlay".to_owned(),
        other => format!("0x{other:x}"),
    })
}

#[cfg(target_os = "macos")]
pub fn statfs_probe(path: &Path) -> Result<String> {
    use std::os::unix::ffi::OsStrExt;

    use anyhow::Context;

    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("path {} contains a NUL byte", path.display()))?;
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a NUL-terminated string and `stat` is a writable statfs buffer.
    let status = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) };
    if status != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("statfs {}", path.display()));
    }
    // SAFETY: the kernel NUL-terminates `f_fstypename`.
    let name = unsafe { std::ffi::CStr::from_ptr(stat.f_fstypename.as_ptr()) };
    Ok(name.to_string_lossy().into_owned())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn statfs_probe(_path: &Path) -> Result<String> {
    Ok("unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refuses_only_the_working_directory(path: &Path) -> Result<String> {
        if path == Path::new(".") {
            Ok("9p".to_owned())
        } else {
            bail!("probed {} instead of the working directory", path.display())
        }
    }

    #[test]
    fn a_fresh_relative_path_probes_the_working_directory() {
        for path in [".corral/db/corral.db", "corral.db", "new/sub/x.db"] {
            let error = match ensure_local(Path::new(path), refuses_only_the_working_directory) {
                Ok(()) => panic!("{path}: admitted"),
                Err(error) => format!("{error:#}"),
            };
            assert!(error.contains(". is on a 9p filesystem"), "{path}: {error}");
        }
    }
}
