//! The scratch foundation (plan T-253 section 8): scratch directories are registered in the
//! store by id, created and deleted only below the validated scratch root through its
//! class-scoped deleter, and reclaimed only from a closed (or due deferred) registration with
//! a crash-safe `BeginReclaim`/`FinishReclaim` pair. Liveness checks (panes, processes, Cargo
//! locks) and their deferral belong to the callers that close attempts.

mod deleter;
mod store_api;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use jiff::Timestamp;

pub use deleter::{ClassDeleter, Registration};
pub use store_api::AttemptScratch;

use crate::backend::{
    Operation, PageReq, ReclaimResult, ScratchKind, ScratchOp, ScratchRow, ScratchState,
    StorageQuery, StorageRow, StoreBackend,
};
use crate::fs_guard::Removal;
use crate::storage_roots::{StorageClass, StorageLayout};

const PAGE: u32 = 256;

/// A new scratch directory's registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewScratch {
    pub id: String,
    pub kind: ScratchKind,
    pub task: Option<String>,
    pub subject: Option<String>,
    pub attempt: Option<String>,
    /// Relative to the scratch root.
    pub path: String,
}

/// What a reclaim did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reclaimed {
    Deleted {
        bytes: u64,
    },
    /// Already gone (a previous attempt deleted it before recording).
    AlreadyAbsent,
}

/// Scratch directories of one workspace, registered in `backend`.
pub struct ScratchArea<'a> {
    backend: &'a dyn StoreBackend,
    deleter: ClassDeleter,
}

impl<'a> ScratchArea<'a> {
    /// Creates the validated scratch root if needed and opens its deleter.
    pub fn open(backend: &'a dyn StoreBackend, layout: &StorageLayout) -> Result<Self> {
        let root = layout.scratch_root();
        crate::fs_guard::create_dir_all_nofollow(root)?;
        Ok(Self {
            backend,
            deleter: ClassDeleter::open(layout, StorageClass::Scratch)?,
        })
    }

    pub fn root(&self) -> &Path {
        self.deleter.root()
    }

    /// Registers `new` (the registration commits first, so whatever is created can be
    /// reclaimed), then creates its directory below the root without following symlinks.
    pub fn register(&self, new: NewScratch, at: Timestamp) -> Result<PathBuf> {
        let entry = Registration {
            class: StorageClass::Scratch,
            id: new.id.clone(),
            rel: PathBuf::from(&new.path),
        };
        self.deleter.preflight(&entry)?;
        self.backend
            .apply(Operation::Scratch(ScratchOp::Register {
                id: new.id,
                kind: new.kind,
                task: new.task,
                subject: new.subject,
                attempt: new.attempt,
                path: new.path,
                at,
            }))
            .map_err(|error| anyhow!("register scratch {}: {error}", entry.id))?;
        // Paired with the nested check a reclaim makes after its BeginReclaim commits: one of
        // the two reads observes the other's committed write, so a nested directory is never
        // created under a parent that is being deleted.
        if let Some(parent) = reclaiming_ancestor(&self.rows()?, &entry) {
            self.close(&entry.id, at)?;
            bail!(
                "scratch {} at {} is at or inside {} ({}), which is being reclaimed; closed it without creating it",
                entry.id,
                entry.rel.display(),
                parent.id,
                parent.path
            );
        }
        self.deleter.create(&entry)
    }

    pub fn close(&self, id: &str, at: Timestamp) -> Result<()> {
        self.backend
            .apply(Operation::Scratch(ScratchOp::Close { id: id.into(), at }))
            .map_err(|error| anyhow!("close scratch {id}: {error}"))?;
        Ok(())
    }

    /// The registration with `id`, if any.
    pub fn row(&self, id: &str) -> Result<Option<ScratchRow>> {
        Ok(self.rows()?.into_iter().find(|row| row.id == id))
    }

    /// Every scratch registration, in every state.
    pub fn rows(&self) -> Result<Vec<ScratchRow>> {
        self.rows_in(&[])
    }

    /// The open registration with `id`, read among open registrations only, so the read is
    /// bounded by work in flight rather than by every directory ever registered.
    pub fn open_row(&self, id: &str) -> Result<Option<ScratchRow>> {
        Ok(self
            .rows_in(&[ScratchState::Open])?
            .into_iter()
            .find(|row| row.id == id))
    }

    /// The registrations in `states` (every state when empty).
    pub fn rows_in(&self, states: &[ScratchState]) -> Result<Vec<ScratchRow>> {
        let mut out = Vec::new();
        let view = self
            .backend
            .view()
            .map_err(|error| anyhow!("open a view: {error}"))?;
        let query = StorageQuery::Scratch {
            states: states.to_vec(),
        };
        let mut page = PageReq::first(PAGE);
        loop {
            let rows = view
                .storage_rows(&query, page)
                .map_err(|error| anyhow!("read scratch registrations: {error}"))?;
            for row in rows.items {
                if let StorageRow::Scratch(row) = row {
                    out.push(row);
                }
            }
            match rows.next {
                Some(after) => {
                    page = PageReq {
                        after: Some(after),
                        limit: PAGE,
                    }
                }
                None => return Ok(out),
            }
        }
    }

    /// Reclaims the registered directory `id`: only that registration's entry below the scratch
    /// root is deleted, through the root handle. It is refused, before `BeginReclaim` and again
    /// immediately before deletion, while another registration not yet reclaimed is at or inside
    /// its path. A reclaim `owner` already began is resumed.
    /// Refusals found before `BeginReclaim` change nothing; a refusal after it records
    /// `failed` with nothing deleted.
    pub fn reclaim(&self, id: &str, owner: &str, at: Timestamp) -> Result<Reclaimed> {
        let row = self
            .row(id)?
            .with_context(|| format!("no scratch directory is registered with id {id:?}"))?;
        let entry = Registration {
            class: StorageClass::Scratch,
            id: row.id.clone(),
            rel: PathBuf::from(&row.path),
        };
        self.deleter.preflight(&entry)?;
        refuse_nested(&self.rows()?, &entry)?;
        let resumed = row.state == ScratchState::Reclaiming && row.owner.as_deref() == Some(owner);
        if !resumed {
            self.backend
                .apply(Operation::Scratch(ScratchOp::BeginReclaim {
                    id: id.into(),
                    from: row.state,
                    owner: owner.into(),
                    at,
                }))
                .map_err(|error| anyhow!("begin reclaiming scratch {id}: {error}"))?;
        }
        let confirm = || -> Result<()> {
            let rows = self.rows()?;
            refuse_nested(&rows, &entry)?;
            match rows.into_iter().find(|now| now.id == id) {
                Some(now)
                    if now.state == ScratchState::Reclaiming
                        && now.owner.as_deref() == Some(owner)
                        && now.path == row.path =>
                {
                    Ok(())
                }
                _ => bail!("scratch {id} is no longer registered for this reclaim"),
            }
        };
        let (result, outcome) = match self.deleter.remove(&entry, &confirm) {
            Ok(Removal::Removed { bytes }) => (
                ReclaimResult::Reclaimed { bytes },
                Ok(Reclaimed::Deleted { bytes }),
            ),
            Ok(Removal::Absent) => (
                ReclaimResult::Reclaimed { bytes: 0 },
                Ok(Reclaimed::AlreadyAbsent),
            ),
            Err(error) => (
                ReclaimResult::Failed {
                    reason: format!("{error:#}"),
                },
                Err(error),
            ),
        };
        self.backend
            .apply(Operation::Scratch(ScratchOp::FinishReclaim {
                id: id.into(),
                owner: owner.into(),
                result,
                at,
            }))
            .map_err(|error| anyhow!("record the reclaim of scratch {id}: {error}"))?;
        outcome
    }
}

/// `inner` equals or lies inside `outer`, by path components.
fn within(inner: &str, outer: &Path) -> bool {
    Path::new(inner).starts_with(outer)
}

/// Refuses reclaiming `entry` while another registration not yet reclaimed is at or inside its
/// path.
fn refuse_nested(rows: &[ScratchRow], entry: &Registration) -> Result<()> {
    if let Some(nested) = rows.iter().find(|row| {
        row.id != entry.id && row.state != ScratchState::Reclaimed && within(&row.path, &entry.rel)
    }) {
        bail!(
            "scratch {} at {} overlaps registration {} ({}, {:?}); refusing to delete it",
            entry.id,
            entry.rel.display(),
            nested.id,
            nested.path,
            nested.state
        );
    }
    Ok(())
}

fn reclaiming_ancestor<'r>(rows: &'r [ScratchRow], entry: &Registration) -> Option<&'r ScratchRow> {
    rows.iter().find(|row| {
        row.id != entry.id
            && row.state == ScratchState::Reclaiming
            && within(&entry.rel.to_string_lossy(), Path::new(&row.path))
    })
}
