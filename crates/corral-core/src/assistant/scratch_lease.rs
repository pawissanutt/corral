//! Admission and scratch GC share this lease, independently of the board writer lock.
use std::fs::OpenOptions;

use anyhow::{Context, Result};

use crate::{Store, StoreLock};

impl Store {
    /// Hold across assistant cwd selection/preparation and startup. Unlike the dispatch lock,
    /// this shared lease may span ordinary assistant updates and artifact preservation.
    pub fn assistant_scratch_lease(&self, id: &str) -> Result<StoreLock> {
        super::validate_id(id)?;
        let dir = self.dir().join("lock");
        std::fs::create_dir_all(&dir).context("creating assistant lease directory")?;
        let path = dir.join(format!("assistant-scratch-{id}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        file.lock_shared()
            .context("leasing assistant scratch admission")?;
        Ok(StoreLock::new(file))
    }

    /// Exclusive nonblocking GC claim. Re-read session/container only after acquiring it.
    pub fn try_assistant_scratch_gc(&self, id: &str) -> Result<Option<StoreLock>> {
        super::validate_id(id)?;
        self.try_lock(&format!("assistant-scratch-{id}"))
    }
}
