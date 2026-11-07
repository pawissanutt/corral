use anyhow::{Context, Result, ensure};
use corral_core::{
    Store,
    blob_ref::{BlobHash, BlobRef},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{io::Read, path::Path};

pub const FILE_MAX: usize = 16 * 1024 * 1024;
pub const TOTAL_MAX: usize = 64 * 1024 * 1024;
pub const FILES_MAX: usize = 4096;
const AREA: &str = "assistant-handoff-content";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Content {
    pub blob: BlobRef,
    pub bytes: usize,
}

impl Content {
    pub fn of(bytes: &[u8]) -> Self {
        Self {
            blob: BlobHash::of(bytes).to_ref(),
            bytes: bytes.len(),
        }
    }
    pub fn put(store: &Store, bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= TOTAL_MAX,
            "snapshot_oversized: content exceeds 64 MiB"
        );
        let reference = Self::of(bytes);
        let key = reference.blob.hash().to_string();
        if let Some(old) = store.record::<Self>(AREA, &key)? {
            ensure!(
                old == reference && old.load(store)? == bytes,
                "recovery_required: immutable handoff content differs"
            );
        } else {
            store.put_evidence(AREA, &key, bytes, |_| json!(reference))?;
        }
        Ok(reference)
    }
    pub fn load(&self, store: &Store) -> Result<Vec<u8>> {
        ensure!(
            self.bytes <= TOTAL_MAX,
            "snapshot_oversized: declared content exceeds 64 MiB"
        );
        store.with_blob_lease(|| {
            let mut bytes = Vec::new();
            store
                .blob_reader(self.blob.hash())?
                .context("snapshot_missing: required handoff content")?
                .take(self.bytes as u64 + 1)
                .read_to_end(&mut bytes)
                .context("read handoff content")?;
            ensure!(
                bytes.len() == self.bytes && BlobHash::of(&bytes) == *self.blob.hash(),
                "snapshot_corrupt: required handoff content {}",
                self.blob.hash()
            );
            Ok(bytes)
        })
    }
}

pub(super) fn regular(path: &Path, max: usize) -> Result<(Vec<u8>, u32)> {
    let mut file = corral_core::fs_guard::open_regular_nofollow(path)
        .with_context(|| format!("snapshot_special_or_missing: {}", path.display()))?;
    let before = file
        .metadata()
        .with_context(|| format!("inspect {}", path.display()))?;
    ensure!(
        before.len() <= max as u64,
        "snapshot_oversized: {} exceeds {max} bytes",
        path.display()
    );
    let mut bytes = Vec::new();
    (&mut file)
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {}", path.display()))?;
    let after = file
        .metadata()
        .with_context(|| format!("reinspect {}", path.display()))?;
    ensure!(
        bytes.len() <= max
            && before.len() == bytes.len() as u64
            && before.modified()? == after.modified()?
            && before.len() == after.len(),
        "snapshot_changed: {} changed while reading",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok((bytes, before.permissions().mode() & 0o777))
    }
    #[cfg(not(unix))]
    {
        anyhow::bail!("snapshot_unsupported: exact file modes require Unix")
    }
}

pub(super) fn safe_path(path: &str) -> Result<()> {
    corral_core::fs_guard::plain_components(Path::new(path))?;
    ensure!(
        !path.contains(['\\', ':'])
            && !path.chars().any(char::is_control)
            && path.split('/').all(|p| !p.eq_ignore_ascii_case(".git")),
        "snapshot_path: unsupported or unsafe path {path:?}"
    );
    Ok(())
}
