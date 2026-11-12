//! The daemon's process-lifetime loop lease is shared by clones, not by separate opens.
use anyhow::Result;

use super::Store;

impl Store {
    /// Claims the non-blocking `daemon` lease until release or the final clone is dropped.
    /// Repeated claims through this handle or its clones are idempotent.
    pub fn own_loop(&self) -> Result<bool> {
        let mut owner = self
            .loop_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if owner.is_some() {
            return Ok(true);
        }
        *owner = self.try_lock("daemon")?;
        Ok(owner.is_some())
    }

    pub fn owns_loop(&self) -> bool {
        self.loop_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    }

    /// A successful probe is dropped immediately; this never waits on another process.
    pub fn loop_owned_elsewhere(&self) -> Result<bool> {
        let owner = self
            .loop_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if owner.is_some() {
            return Ok(false);
        }
        Ok(self.try_lock("daemon")?.is_none())
    }

    pub fn release_loop(&self) {
        self.loop_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separately_opened_store_detects_loop_owner_until_release() {
        let dir = tempfile::tempdir().unwrap();
        let first = Store::init(dir.path()).unwrap();
        let second = Store::open(dir.path()).unwrap();
        assert!(first.own_loop().unwrap());
        assert!(first.clone().own_loop().unwrap());
        assert!(first.clone().owns_loop());
        assert!(!first.loop_owned_elsewhere().unwrap());
        assert!(second.loop_owned_elsewhere().unwrap());
        assert!(!second.own_loop().unwrap());
        first.clone().release_loop();
        assert!(!first.owns_loop());
        assert!(!second.loop_owned_elsewhere().unwrap());
        // A successful probe did not retain the lease.
        assert!(first.own_loop().unwrap());
    }
}
