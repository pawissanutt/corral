//! Database fences: legacy scheduler 4, independent capacity scheduler 5.
use anyhow::{Result, bail};

use crate::Store;

/// Highest supported database fence. Fresh stores stay at 4 until explicit capacity activation;
/// legacy file stores carry 2 or 3.
pub const STORE_SCHEMA_VERSION: u32 = 5;

pub(crate) fn store_version(store: &Store) -> Result<u32> {
    crate::work::record(crate::work::Work::SchemaRead, 1);
    match crate::store::db::fence(store.dir())?.as_deref() {
        Some("4") => Ok(4),
        Some("5") => Ok(5),
        Some(other) => bail!("unsupported store schema {other}; restart with a compatible corral"),
        None => bail!("the store fence is missing; restart with a compatible corral"),
    }
}
