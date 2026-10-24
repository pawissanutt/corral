//! Blob garbage collection (plan T-253 section 7), run by the one holder of the `storage-gc`
//! claim.
//!
//! A pass first finishes rows a dead holder left `deleting`, then deletes every blob with no
//! references and no live reservation that has been unreferenced since before the horizon
//! (`now - grace`, or the start of the oldest live lease if earlier), then stray files older
//! than the grace period. Each deletion is `BeginDelete`, unlink, `FinishDelete`; a blob a
//! putter reserved or an operation referenced after selection fails `BeginDelete` and is kept.

use std::time::Duration;

use anyhow::{Result, anyhow};
use jiff::{SignedDuration, Timestamp};
use serde::Serialize;

use super::holder::{Acquired, GcClaim, ProcessIdentity};
use super::{PAGE, all_pages, blob_row, rows_in};
use crate::backend::{
    ApplyError, BlobOp, BlobRow, BlobState, BlobStore, Clock, Operation, StorageQuery, StorageRow,
    StoreBackend,
};
use crate::blob_ref::BlobHash;

/// How long an unreferenced blob stays readable through a `BlobRef` read before it.
pub const BLOB_GRACE: Duration = Duration::from_secs(24 * 3600);

/// A point inside a pass where a barrier test can act; an error from the barrier ends the pass.
#[derive(Debug, Clone, Copy)]
pub enum GcPoint<'a> {
    /// Chosen for deletion; `BeginDelete` comes next.
    Selected(&'a BlobHash),
    /// The row is `deleting`; the unlink comes next.
    Deleting(&'a BlobHash),
    /// The file is gone; `FinishDelete` comes next.
    Unlinked(&'a BlobHash),
}

pub type GcBarrier<'a> = &'a dyn Fn(GcPoint<'_>) -> Result<()>;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct GcReport {
    pub horizon: Option<Timestamp>,
    /// Deletions a previous holder left unfinished.
    pub completed: Vec<BlobHash>,
    pub deleted: Vec<BlobHash>,
    pub strays: Vec<BlobHash>,
    /// Selected, then refused by `BeginDelete` because a putter or a reference got there first.
    pub kept: Vec<KeptBlob>,
    pub freed_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeptBlob {
    pub hash: BlobHash,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GcRun {
    Done(GcReport),
    /// A live process holds the claim; nothing was done.
    ClaimHeld {
        owner: String,
    },
}

pub struct Gc<'a> {
    backend: &'a dyn StoreBackend,
    blobs: &'a dyn BlobStore,
    clock: &'a dyn Clock,
    me: ProcessIdentity,
    grace: Duration,
    barrier: Option<GcBarrier<'a>>,
}

impl<'a> Gc<'a> {
    pub fn new(
        backend: &'a dyn StoreBackend,
        blobs: &'a dyn BlobStore,
        clock: &'a dyn Clock,
    ) -> Result<Self> {
        Ok(Self {
            backend,
            blobs,
            clock,
            me: ProcessIdentity::current()?,
            grace: BLOB_GRACE,
            barrier: None,
        })
    }

    pub fn grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    pub fn barrier(mut self, barrier: GcBarrier<'a>) -> Self {
        self.barrier = Some(barrier);
        self
    }

    /// Takes the claim (from a holder proven dead if need be), runs one pass and releases it.
    pub fn run(&self) -> Result<GcRun> {
        let claim = GcClaim {
            backend: self.backend,
            clock: self.clock,
            me: &self.me,
        };
        if let Acquired::Held { owner } = claim.acquire()? {
            return Ok(GcRun::ClaimHeld { owner });
        }
        let pass = self.pass();
        let released = claim.release();
        let report = pass?;
        released?;
        Ok(GcRun::Done(report))
    }

    fn pass(&self) -> Result<GcReport> {
        let owner = self.me.owner();
        let mut report = GcReport::default();
        for row in rows_in(self.backend, Some(BlobState::Deleting))? {
            finish_deletion(self.backend, self.blobs, &row.hash, &owner)?;
            report.freed_bytes += row.stored_bytes;
            report.completed.push(row.hash);
        }

        let now = self.clock.now();
        let grace_start = now.checked_sub(SignedDuration::try_from(self.grace)?)?;
        let horizon = self
            .oldest_live_lease(now)?
            .map_or(grace_start, |started| started.min(grace_start));
        report.horizon = Some(horizon);
        let candidates: Vec<BlobRow> = rows_in(self.backend, None)?
            .into_iter()
            .filter(|row| eligible(row, now, horizon))
            .collect();
        for row in candidates {
            self.point(GcPoint::Selected(&row.hash))?;
            let begin = Operation::Blob(BlobOp::BeginDelete {
                hash: row.hash.clone(),
                gc_owner: owner.clone(),
                horizon,
            });
            match self.backend.apply(begin) {
                Ok(_) => {}
                Err(ApplyError::Precondition(reason)) => {
                    report.kept.push(KeptBlob {
                        hash: row.hash,
                        reason,
                    });
                    continue;
                }
                Err(error) => return Err(anyhow!("begin deleting blob {}: {error}", row.hash)),
            }
            self.point(GcPoint::Deleting(&row.hash))?;
            self.blobs.delete(&row.hash)?;
            self.point(GcPoint::Unlinked(&row.hash))?;
            finish(self.backend, &row.hash, &owner)?;
            report.freed_bytes += row.stored_bytes;
            report.deleted.push(row.hash);
        }

        self.strays(grace_start, &owner, &mut report)?;
        Ok(report)
    }

    fn strays(&self, before: Timestamp, owner: &str, report: &mut GcReport) -> Result<()> {
        let mut after: Option<BlobHash> = None;
        loop {
            let files = self.blobs.list(after.as_ref(), PAGE)?;
            let Some(last) = files.last() else {
                return Ok(());
            };
            after = Some(last.hash.clone());
            let full = files.len() == PAGE as usize;
            for file in files.into_iter().filter(|file| file.modified < before) {
                let has_row = {
                    let view = self.backend.view()?;
                    blob_row(view.as_ref(), &file.hash)?.is_some()
                };
                if has_row {
                    continue;
                }
                let adopt = Operation::Blob(BlobOp::AdoptStray {
                    hash: file.hash.clone(),
                    gc_owner: owner.to_owned(),
                    stored_bytes: file.stored_bytes,
                });
                match self.backend.apply(adopt) {
                    Ok(_) => {}
                    Err(ApplyError::Precondition(_)) => continue,
                    Err(error) => return Err(anyhow!("adopt stray blob {}: {error}", file.hash)),
                }
                finish_deletion(self.backend, self.blobs, &file.hash, owner)?;
                report.freed_bytes += file.stored_bytes;
                report.strays.push(file.hash);
            }
            if !full {
                return Ok(());
            }
        }
    }

    fn oldest_live_lease(&self, now: Timestamp) -> Result<Option<Timestamp>> {
        let view = self.backend.view()?;
        let rows = all_pages(|page| view.storage_rows(&StorageQuery::Leases, page))?;
        Ok(rows
            .into_iter()
            .filter_map(|row| match row {
                StorageRow::Lease(lease) if lease.expires > now => Some(lease.started),
                _ => None,
            })
            .min())
    }

    fn point(&self, point: GcPoint<'_>) -> Result<()> {
        self.barrier.map_or(Ok(()), |barrier| barrier(point))
    }
}

fn eligible(row: &BlobRow, now: Timestamp, horizon: Timestamp) -> bool {
    matches!(row.state, BlobState::Live | BlobState::Writing)
        && row.refs == 0
        && row.reservations.iter().all(|r| r.expires <= now)
        && row.unreferenced_since.unwrap_or(row.created) < horizon
}

fn finish(backend: &dyn StoreBackend, hash: &BlobHash, owner: &str) -> Result<()> {
    let finish = Operation::Blob(BlobOp::FinishDelete {
        hash: hash.clone(),
        gc_owner: owner.to_owned(),
    });
    backend
        .apply(finish)
        .map(drop)
        .map_err(|error| anyhow!("finish deleting blob {hash}: {error}"))
}

/// Unlinks `hash` and removes its `deleting` row; the caller holds the claim as `owner`.
pub(super) fn finish_deletion(
    backend: &dyn StoreBackend,
    blobs: &dyn BlobStore,
    hash: &BlobHash,
    owner: &str,
) -> Result<()> {
    blobs.delete(hash)?;
    finish(backend, hash, owner)
}
