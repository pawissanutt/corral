//! The guard of one advisory store lock.
use std::fs;

/// Release the advisory lock at the operation boundary, even if a concurrent
/// fork still holds a duplicate of the open-file description.
#[derive(Debug)]
pub struct StoreLock(pub(super) fs::File);

impl StoreLock {
    pub(crate) fn new(file: fs::File) -> StoreLock {
        StoreLock(file)
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            crate::messages::report(None, "store lock", format!("releasing store lock: {error}"));
        }
    }
}
