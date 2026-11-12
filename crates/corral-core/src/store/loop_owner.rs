//! The daemon's process-lifetime loop lease is shared by clones, not by separate opens.
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use super::{Store, StoreLock};

/// Keeps a pass covered by the lifetime owner or a shared lease until it finishes.
#[derive(Debug)]
pub enum LoopPass {
    Owned,
    Shared(StoreLock),
}

impl Store {
    /// Claims the exclusive `daemon` lease until release or the final clone is dropped.
    /// Returns false immediately for another daemon; waits at most 120 seconds for
    /// existing shared sync passes, then reports an error if they have not finished.
    /// Repeated claims through this handle or its clones are idempotent.
    pub fn own_loop(&self) -> Result<bool> {
        self.own_loop_with_timeout(Duration::from_secs(120))
    }

    pub(crate) fn own_loop_with_timeout(&self, timeout: Duration) -> Result<bool> {
        let started = Instant::now();
        loop {
            {
                let mut owner = self
                    .loop_owner
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if owner.is_some() {
                    return Ok(true);
                }
                if let Some(lease) = self.try_lock("daemon")? {
                    *owner = Some(lease);
                    return Ok(true);
                }
                let Some(probe) = self.try_lock_shared("daemon")? else {
                    return Ok(false);
                };
                drop(probe);
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                bail!(
                    "a sync pass is still running; daemon loop lease timed out after {timeout:?}"
                );
            }
            std::thread::sleep(remaining.min(Duration::from_millis(25)));
        }
    }

    pub fn owns_loop(&self) -> bool {
        self.loop_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    }

    /// Acquires a shared lease for the complete pass, or returns None when fenced.
    /// The lifetime owner's clones must not open a conflicting second descriptor.
    pub fn loop_pass(&self) -> Result<Option<LoopPass>> {
        let owner = self
            .loop_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if owner.is_some() {
            return Ok(Some(LoopPass::Owned));
        }
        Ok(self.try_lock_shared("daemon")?.map(LoopPass::Shared))
    }

    /// True only when another daemon holds the exclusive lease, not for shared passes.
    /// A successful shared probe is dropped immediately and never waits on the lock.
    pub fn loop_owned_elsewhere(&self) -> Result<bool> {
        Ok(self.loop_pass()?.is_none())
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
        assert!(matches!(first.loop_pass().unwrap(), Some(LoopPass::Owned)));
        assert!(second.loop_pass().unwrap().is_none());
        assert!(second.loop_owned_elsewhere().unwrap());
        assert!(!second.own_loop().unwrap());
        first.clone().release_loop();
        assert!(!first.owns_loop());
        assert!(!second.loop_owned_elsewhere().unwrap());
        // A successful probe did not retain the lease.
        assert!(first.own_loop().unwrap());
    }

    #[test]
    fn daemon_cannot_start_until_all_in_flight_shared_passes_finish() {
        let dir = tempfile::tempdir().unwrap();
        let first = Store::init(dir.path()).unwrap();
        let second = Store::open(dir.path()).unwrap();
        let daemon = Store::open(dir.path()).unwrap();
        let first_pass = first.loop_pass().unwrap().unwrap();
        let second_pass = second.loop_pass().unwrap().unwrap();
        assert!(!daemon.loop_owned_elsewhere().unwrap());
        assert!(
            daemon
                .own_loop_with_timeout(Duration::from_millis(1))
                .is_err()
        );
        assert!(!daemon.owns_loop());
        drop(first_pass);
        assert!(
            daemon
                .own_loop_with_timeout(Duration::from_millis(1))
                .is_err()
        );
        drop(second_pass);
        assert!(
            daemon
                .own_loop_with_timeout(Duration::from_millis(1))
                .unwrap()
        );
    }
}
