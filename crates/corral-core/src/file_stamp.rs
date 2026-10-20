//! When an unchanged file may be served from a cache without reading it again.
//!
//! A stamp is the full stat identity (device, inode, size, mtime, ctime). It stands in for the
//! bytes only if it was taken before they were read and the file had already settled then: its
//! last change was more than a settle window older than the observation. The kernel sets ctime
//! to the current time on every write, rename and timestamp change, and user space cannot set
//! it, so any later change, even a same-size rewrite that restores mtime, yields a different
//! stamp. Files changed within the window ("racily clean", as git calls it) and platforms
//! without ctime must be read and compared again.

use std::fs::Metadata;
use std::time::{Duration, SystemTime};

/// Far above any filesystem timestamp granularity and small clock adjustments.
pub const SETTLE_WINDOW: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl FileStamp {
    /// `None` where ctime is unavailable: callers then always read.
    #[cfg(unix)]
    pub fn of(metadata: &Metadata) -> Option<FileStamp> {
        use std::os::unix::fs::MetadataExt;
        Some(FileStamp {
            dev: metadata.dev(),
            ino: metadata.ino(),
            len: metadata.len(),
            mtime: (metadata.mtime(), metadata.mtime_nsec()),
            ctime: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }

    #[cfg(not(unix))]
    pub fn of(_: &Metadata) -> Option<FileStamp> {
        None
    }

    /// The stamp of the metadata, kept only if it had settled `window` before `observed`.
    pub fn settled(metadata: &Metadata, observed: SystemTime, window: Duration) -> Option<Self> {
        Self::of(metadata).filter(|stamp| {
            let changed = stamp.mtime.max(stamp.ctime);
            let (Ok(secs), Ok(nanos)) = (u64::try_from(changed.0), u32::try_from(changed.1)) else {
                return false;
            };
            observed
                .duration_since(SystemTime::UNIX_EPOCH + Duration::new(secs, nanos))
                .is_ok_and(|age| age > window)
        })
    }
}
