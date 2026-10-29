//! `storage check` and `storage backup` (plan T-253 section 7).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::Store;
use corral_core::backend::{BlobQuery, BlobState, BlobStore as _, PageReq, StoreBackend as _};
use corral_core::blob::FsBlobStore;
use corral_core::sqlite::{SqliteBackend, SqliteOptions};
use serde::Serialize;

/// The result of `storage check`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StorageCheck {
    /// Database integrity problems.
    pub database: Vec<String>,
    pub blobs_checked: u64,
    /// Live blobs that are missing or fail their hash.
    pub blobs: Vec<String>,
    /// Unreferenced blobs whose content is already gone (collectable, not damage).
    pub unreferenced_missing: Vec<String>,
}

impl StorageCheck {
    pub fn is_ok(&self) -> bool {
        self.database.is_empty() && self.blobs.is_empty()
    }
}

/// What `storage backup` copied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupReport {
    pub dir: PathBuf,
    /// The change sequence of the database copy.
    pub change_seq: u64,
    pub blobs: u64,
    pub blob_bytes: u64,
}

/// Database integrity, then every live blob read back to its hash.
pub fn check_storage(store: &Store) -> Result<StorageCheck> {
    let mut check = StorageCheck {
        database: store.check_database()?,
        ..StorageCheck::default()
    };
    let blobs = store.blob_store()?;
    for row in store.blob_rows()? {
        if row.state != BlobState::Live {
            continue;
        }
        check.blobs_checked += 1;
        match blobs.get(&row.hash) {
            Err(_) if row.refs == 0 => check.unreferenced_missing.push(row.hash.to_string()),
            Err(error) => check
                .blobs
                .push(format!("blob {} is missing: {error:#}", row.hash)),
            Ok(mut reader) => {
                if let Err(error) = std::io::copy(&mut reader, &mut std::io::sink()) {
                    check
                        .blobs
                        .push(format!("blob {} fails verification: {error}", row.hash));
                }
            }
        }
    }
    Ok(check)
}

fn copy_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::copy(from, to).with_context(|| format!("copying {}", from.display()))?;
    Ok(())
}

/// Under a storage lease, so blob GC deletes nothing the copy references: one consistent
/// database copy into `<dir>/.corral/db`, then every live blob of that copy, each read back to
/// its hash, plus the fence, the pins and the configuration with project paths made absolute,
/// so `corral --root <dir>` opens the copy. `<dir>/.corral` must not exist.
pub fn backup_storage(store: &Store, dir: &Path) -> Result<BackupReport> {
    let corral = dir.join(".corral");
    if fs::symlink_metadata(&corral).is_ok() {
        bail!(
            "{} already exists; back up into a new directory",
            corral.display()
        );
    }
    fs::create_dir_all(&corral).with_context(|| format!("creating {}", corral.display()))?;
    store.with_blob_lease(|| {
        let db = SqliteBackend::db_path(&corral);
        if let Some(parent) = db.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        store.backup_database(&db)?;
        let copy = SqliteBackend::open(&corral, SqliteOptions::default())?;
        let view = copy.view()?;
        let change_seq = view.seq().0;
        let mut rows = Vec::new();
        let mut req = PageReq::first(256);
        loop {
            let page = view.blobs(&BlobQuery::default(), req)?;
            rows.extend(page.items);
            let Some(after) = page.next else { break };
            req = PageReq {
                after: Some(after),
                limit: 256,
            };
        }
        drop(view);
        let source = store.blob_store()?;
        let dest = FsBlobStore::open(&corral)?;
        let mut report = BackupReport {
            dir: dir.to_path_buf(),
            change_seq,
            blobs: 0,
            blob_bytes: 0,
        };
        for row in rows.into_iter().filter(|row| row.state == BlobState::Live) {
            let from = source.path_of(&row.hash);
            // Only an unreferenced blob can be collected after the lease started; the copy
            // loses nothing it references.
            if row.refs == 0 && !from.is_file() {
                continue;
            }
            copy_file(&from, &dest.path_of(&row.hash))?;
            std::io::copy(&mut dest.get(&row.hash)?, &mut std::io::sink())
                .with_context(|| format!("verifying the copy of blob {}", row.hash))?;
            report.blobs += 1;
            report.blob_bytes += row.stored_bytes;
        }
        let fence = Path::new("state").join("schema-version");
        for rel in [Path::new("pins.toml"), fence.as_path()] {
            let from = store.dir().join(rel);
            if from.is_file() {
                copy_file(&from, &corral.join(rel))?;
            }
        }
        let config = store.config_with_absolute_projects()?;
        fs::write(corral.join("corral.toml"), config)
            .context("writing the backup configuration")?;
        Ok(report)
    })
}
