//! Reuse unchanged event snapshots without changing Store's public read contract.

use std::collections::HashMap;
use std::fs::{self, Metadata};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow};
use corral_core::{Event, Store};

static EVENTS: OnceLock<Mutex<HashMap<PathBuf, Snapshot>>> = OnceLock::new();

struct Snapshot {
    version: Option<Version>,
    events: Arc<[Event]>,
}

#[derive(PartialEq, Eq)]
struct Version {
    identity: String,
    len: u64,
    modified: SystemTime,
    #[cfg(unix)]
    changed: (i64, i64),
}

impl Version {
    fn read(metadata: &Metadata) -> Result<Self> {
        Ok(Self {
            identity: super::transcript_cache::identity(metadata),
            len: metadata.len(),
            modified: metadata
                .modified()
                .context("reading event log modification time")?,
            #[cfg(unix)]
            changed: {
                use std::os::unix::fs::MetadataExt;
                (metadata.ctime(), metadata.ctime_nsec())
            },
        })
    }
}

pub(super) fn events(store: &Store) -> Result<Arc<[Event]>> {
    let path = store.dir().join("events.jsonl");
    let version = match fs::metadata(&path) {
        Ok(metadata) => Some(Version::read(&metadata)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let mut cache = EVENTS
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| anyhow!("event snapshot cache lock poisoned"))?;
    if let Some(snapshot) = cache.get(&path)
        && snapshot.version == version
    {
        return Ok(Arc::clone(&snapshot.events));
    }
    #[cfg(test)]
    super::EVENT_READS.set(super::EVENT_READS.get() + 1);
    let events: Arc<[Event]> = store.events()?.into();
    cache.insert(
        path,
        Snapshot {
            version,
            events: Arc::clone(&events),
        },
    );
    Ok(events)
}
