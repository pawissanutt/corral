//! The guard of one advisory store lock.
use std::fs;
use std::path::{Path, PathBuf};

/// Release the advisory lock at the operation boundary, even if a concurrent
/// fork still holds a duplicate of the open-file description.
#[derive(Debug)]
pub struct StoreLock(pub(super) fs::File, Option<PathBuf>, bool);

impl StoreLock {
    pub(crate) fn new(file: fs::File) -> StoreLock {
        StoreLock(file, None, false)
    }

    pub(crate) fn for_path(file: fs::File, path: PathBuf, shared: bool) -> StoreLock {
        StoreLock(file, Some(path), shared)
    }

    /// Validate a caller-supplied claim without taking a second, non-reentrant lock.
    /// Anonymous and shared guards never authorize an exclusive claimed operation.
    pub fn exclusively_protects(&self, path: &Path) -> bool {
        !self.2 && self.1.as_deref() == Some(path)
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            crate::messages::report(None, "store lock", format!("releasing store lock: {error}"));
        }
    }
}
