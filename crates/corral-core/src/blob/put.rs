//! The put protocol (plan T-253 section 7): stage, `Reserve`, publish, and the referencing
//! operation consumes the reservation.

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use super::blob_row;
use super::gc::finish_deletion;
use super::holder::{Acquired, GcClaim, ProcessIdentity};
use crate::backend::{
    BlobOp, BlobState, BlobStore, Clock, Operation, Outcome, ReservationUse, ReserveOutcome,
    Staged, StagedData, StoreBackend,
};
use crate::blob_ref::{BlobHash, BlobRef};

/// How long a reservation keeps GC away from a blob that no operation references yet.
pub const RESERVATION_TTL: Duration = Duration::from_secs(3600);

/// A blob ready to be referenced: put `reservation` in the referencing operation's
/// `reservations` and `blob` in its document.
#[derive(Debug, Clone, PartialEq)]
pub struct PutBlob {
    pub blob: BlobRef,
    pub reservation: ReservationUse,
    pub raw_bytes: u64,
    pub stored_bytes: u64,
    pub codec: String,
    /// This put inserted the row (`MustWrite`) rather than finding it.
    pub created: bool,
}

#[derive(Debug)]
pub enum TryPut {
    Reserved(PutBlob),
    /// GC is deleting this hash; the staged content is handed back for a later attempt.
    Busy(Staged),
}

pub struct Putter<'a> {
    backend: &'a dyn StoreBackend,
    blobs: &'a dyn BlobStore,
    clock: &'a dyn Clock,
    me: ProcessIdentity,
    owner: String,
}

impl<'a> Putter<'a> {
    pub fn new(
        backend: &'a dyn StoreBackend,
        blobs: &'a dyn BlobStore,
        clock: &'a dyn Clock,
    ) -> Result<Self> {
        let me = ProcessIdentity::current()?;
        let owner = me.owner();
        Ok(Self {
            backend,
            blobs,
            clock,
            me,
            owner,
        })
    }

    /// Reserves the staged hash and publishes its content unless GC is deleting it.
    pub fn try_put(&self, staged: Staged) -> Result<TryPut> {
        let token = self.token();
        let reserve = Operation::Blob(BlobOp::Reserve {
            hash: staged.hash.clone(),
            raw_bytes: staged.raw_bytes,
            stored_bytes: staged.stored_bytes,
            codec: staged.codec.clone(),
            token: token.clone(),
            owner: self.owner.clone(),
            ttl: RESERVATION_TTL,
        });
        let outcome = match self.backend.apply(reserve) {
            Ok(committed) => committed.outcome,
            Err(error) => {
                let hash = staged.hash.clone();
                discard(staged);
                return Err(anyhow!("reserve blob {hash}: {error}"));
            }
        };
        let reserved = match outcome {
            Outcome::Reserved(ReserveOutcome::Busy) => return Ok(TryPut::Busy(staged)),
            Outcome::Reserved(reserved) => reserved,
            other => {
                discard(staged);
                bail!("reserve returned {other:?}");
            }
        };
        let blob = PutBlob {
            blob: staged.hash.to_ref(),
            reservation: ReservationUse {
                hash: staged.hash.clone(),
                token,
            },
            raw_bytes: staged.raw_bytes,
            stored_bytes: staged.stored_bytes,
            codec: staged.codec.clone(),
            created: reserved == ReserveOutcome::MustWrite,
        };
        let hash = staged.hash.clone();
        self.blobs
            .put_staged(staged)
            .with_context(|| format!("publish blob {hash}"))?;
        Ok(TryPut::Reserved(blob))
    }

    /// Stages `src` and puts it. While GC is deleting the hash, `wait(attempt)` runs between
    /// attempts (an error from it ends the put); a GC holder proven dead is taken over and its
    /// deletion finished first.
    pub fn put(
        &self,
        src: &mut dyn Read,
        wait: &mut dyn FnMut(u32) -> Result<()>,
    ) -> Result<PutBlob> {
        let mut staged = self.blobs.stage(src)?;
        let mut attempt = 0;
        loop {
            match self.try_put(staged)? {
                TryPut::Reserved(blob) => return Ok(blob),
                TryPut::Busy(back) => staged = back,
            }
            let taken_over = self.take_over(&staged.hash);
            match taken_over {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => {
                    discard(staged);
                    return Err(error);
                }
            }
            attempt += 1;
            if let Err(error) = wait(attempt) {
                discard(staged);
                return Err(error);
            }
        }
    }

    /// With the claim free or its holder proven dead, finishes the deletion of `hash` so the
    /// next reserve can rewrite it. False while a live holder keeps the claim.
    fn take_over(&self, hash: &BlobHash) -> Result<bool> {
        let claim = GcClaim {
            backend: self.backend,
            clock: self.clock,
            me: &self.me,
        };
        if let Acquired::Held { .. } = claim.acquire()? {
            return Ok(false);
        }
        let finished = self.finish_if_deleting(hash);
        let released = claim.release();
        finished?;
        released?;
        Ok(true)
    }

    fn finish_if_deleting(&self, hash: &BlobHash) -> Result<()> {
        let deleting = {
            let view = self.backend.view()?;
            blob_row(view.as_ref(), hash)?.is_some_and(|row| row.state == BlobState::Deleting)
        };
        if deleting {
            finish_deletion(self.backend, self.blobs, hash, &self.owner)?;
        }
        Ok(())
    }

    fn token(&self) -> String {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        format!("{}-{n}", self.owner)
    }
}

/// Removes the staging file of content that will not be published. A removal this misses is
/// left for `FsBlobStore::sweep_staging`; the caller's own error is what it reports.
fn discard(staged: Staged) {
    if let StagedData::File(path) = staged.data {
        let _ = std::fs::remove_file(path);
    }
}
