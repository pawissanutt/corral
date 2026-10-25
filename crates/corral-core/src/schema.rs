//! The store fence (`state/schema-version`): a database store reads and writes only fence 4.
use anyhow::{Result, bail};

use crate::Store;

/// The fence value of a database store; legacy file stores carry 2 or 3.
pub const STORE_SCHEMA_VERSION: u32 = 4;

pub(crate) fn store_version(store: &Store) -> Result<u32> {
    crate::work::record(crate::work::Work::SchemaRead, 1);
    match crate::store::db::fence(store.dir())?.as_deref() {
        Some("4") => Ok(STORE_SCHEMA_VERSION),
        Some(other) => bail!("unsupported store schema {other}; restart with a compatible corral"),
        None => bail!("the store fence is missing; restart with a compatible corral"),
    }
}
