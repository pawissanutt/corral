//! Raw host context is a private, disposable cache of verified compressed evidence.
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use corral_core::{Store, blob_ref::BlobHash};

use super::ContinuationIntent;

fn directory(store: &Store, intent: &ContinuationIntent) -> PathBuf {
    store
        .dir()
        .join("continuation-context")
        .join(corral_core::agent::paths::sha256_hex(
            intent.link.episode.as_bytes(),
        ))
}

pub(super) fn materialize(
    store: &Store,
    intent: &ContinuationIntent,
    checkpoint: &super::checkpoint::ContinuationCheckpoint,
    bytes: &[u8],
) -> Result<PathBuf> {
    ensure!(
        bytes.len() == checkpoint.context.bytes
            && BlobHash::of(bytes) == *checkpoint.context.blob.hash(),
        "CheckpointInvalid: verified context bytes changed"
    );
    let dir = directory(store, intent);
    #[cfg(unix)]
    {
        let root = store.dir().join("continuation-context");
        crate::agent::launch::ensure_private_dir(&root, &dir)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(&dir).context("creating private continuation context directory")?;
    let path = dir.join(checkpoint.context.blob.hash().as_str());
    // Always republish from canonical bytes: a missing or corrupted cache is not authority.
    // NamedTempFile starts at 0600; persist replaces the destination atomically.
    let mut file = tempfile::NamedTempFile::new_in(&dir)
        .context("creating private continuation context cache")?;
    file.write_all(bytes)
        .context("writing verified continuation context cache")?;
    file.as_file()
        .sync_all()
        .context("syncing verified continuation context cache")?;
    file.persist(&path)
        .context("publishing verified continuation context cache")?;
    Ok(path)
}

pub(super) fn remove(store: &Store, intent: &ContinuationIntent) -> Result<()> {
    match std::fs::remove_dir_all(directory(store, intent)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("removing closed continuation context cache"),
    }
}
