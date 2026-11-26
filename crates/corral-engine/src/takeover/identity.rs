//! The identity observed before a worker starts, not the directory found by a later takeover.
use anyhow::Result;

use crate::{Run, Store};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS run_checkout_identities (
    task TEXT NOT NULL, run TEXT NOT NULL, document TEXT NOT NULL,
    PRIMARY KEY(task,run),
    FOREIGN KEY (task,run) REFERENCES runs(task,id) ON DELETE CASCADE
);";

#[cfg(unix)]
use {
    anyhow::Context as _,
    serde::{Deserialize, Serialize},
    std::{fs, os::unix::fs::MetadataExt as _, path::Path},
};

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct EntryIdentity {
    device: u64,
    inode: u64,
    // Inodes can be recycled. Require a birth time as well as the inode, or
    // leave the run unverified on filesystems which cannot establish it.
    created: jiff::Timestamp,
}

#[cfg(unix)]
impl EntryIdentity {
    fn from_metadata(meta: fs::Metadata) -> Option<Self> {
        Some(Self {
            device: meta.dev(),
            inode: meta.ino(),
            created: meta.created().ok()?.try_into().ok()?,
        })
    }
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CheckoutIdentity {
    directory: EntryIdentity,
    git_pointer: EntryIdentity,
}

#[cfg(not(unix))]
pub(crate) struct CheckoutIdentity;

impl CheckoutIdentity {
    pub(crate) fn capture_worker(run: &Run) -> Option<Self> {
        #[cfg(unix)]
        if run.role == crate::Role::Worker {
            return Self::capture(&run.worktree);
        }
        #[cfg(not(unix))]
        let _ = run;
        None
    }

    #[cfg(unix)]
    pub(super) fn capture(path: &Path) -> Option<Self> {
        if path.canonicalize().ok()? != path {
            return None;
        }
        let directory = fs::symlink_metadata(path).ok()?;
        let git_pointer = fs::symlink_metadata(path.join(".git")).ok()?;
        if !directory.is_dir() || !git_pointer.is_file() {
            return None;
        }
        Some(Self {
            directory: EntryIdentity::from_metadata(directory)?,
            git_pointer: EntryIdentity::from_metadata(git_pointer)?,
        })
    }

    #[cfg(unix)]
    pub(super) fn directory_identity(&self) -> (String, String) {
        (
            self.directory.device.to_string(),
            self.directory.inode.to_string(),
        )
    }

    pub(crate) fn record(&self, store: &Store, run: &Run) -> Result<()> {
        #[cfg(unix)]
        store
            .connection
            .execute(
                "INSERT INTO run_checkout_identities(task,run,document) VALUES (?1,?2,?3)",
                rusqlite::params![run.task, run.id, crate::store::encode(self)?],
            )
            .context("register worker's checkout identity")?;
        #[cfg(not(unix))]
        let _ = (store, run);
        Ok(())
    }

    #[cfg(unix)]
    pub(super) fn load(store: &Store, run: &Run) -> Result<Option<Self>> {
        // The read-only host CLI can be the first upgraded component to open a
        // legacy store. A missing table is not permission to adopt its checkout.
        let registered: bool = store.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='run_checkout_identities' AND type='table')",
            [], |row| row.get(0),
        ).context("check for checkout identity support")?;
        if !registered {
            return Ok(None);
        }
        store.optional_document(
            "SELECT document FROM run_checkout_identities WHERE task=?1 AND run=?2",
            rusqlite::params![run.task, run.id],
        )
    }
}
