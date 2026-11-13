//! A disposable index failure is repaired once, without swallowing canonical-source errors.
use std::fmt;

use anyhow::{Context, Result};

use crate::Ctx;

#[derive(Debug)]
pub(super) struct Corrupt;

impl fmt::Display for Corrupt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("incomplete or corrupt search catalog")
    }
}
impl std::error::Error for Corrupt {}

pub(crate) fn is_corrupt(error: &anyhow::Error) -> bool {
    error.is::<Corrupt>()
}

pub(crate) fn repair(ctx: &Ctx) -> Result<()> {
    // Published partitions are immutable; invalidation needs no serving-index lock.
    let root = ctx.store.cache_root()?.join("search-catalog");
    std::fs::create_dir_all(&root).context("creating catalog repair directory")?;
    crate::artifact_storage::replace(&root.join("restart"), b"canonical")?;
    let path = root.join("manifest.json");
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("invalidating derived search catalog"),
    }
}
