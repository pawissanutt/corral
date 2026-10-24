//! Evidence blobs (plan T-253 section 7): the filesystem [`FsBlobStore`], the put protocol
//! ([`Putter`]), garbage collection under the `storage-gc` claim ([`Gc`]) and the content check
//! ([`check_blobs`]).
//!
//! A put stages content (streamed sha256 and zstd), reserves its hash, publishes the file and
//! hands the reservation to the operation that references the blob, which turns it live. GC
//! deletes only unreferenced, unreserved blobs older than its horizon, one at a time through
//! `BeginDelete`, unlink and `FinishDelete`; `Reserve` and `BeginDelete` conflict on the row, so
//! a put and a deletion never interleave. Only the claim holder unlinks, and another process
//! takes the claim over only after proving the holder dead by pid and start time.

mod check;
mod fs_store;
mod gc;
mod holder;
mod put;

use anyhow::Result;

use crate::backend::{
    BlobQuery, BlobRow, BlobState, Page, PageReq, ReadView, StoreBackend, ViewError,
};
use crate::blob_ref::BlobHash;

pub use check::{BlobCheck, BlobProblem, check_blobs};
pub use fs_store::{CODEC, FsBlobStore, SyncHook, SyncTarget};
pub use gc::{BLOB_GRACE, Gc, GcBarrier, GcPoint, GcReport, GcRun, KeptBlob};
pub use holder::{ProcessIdentity, proven_dead};
pub use put::{PutBlob, Putter, RESERVATION_TTL, TryPut};

const PAGE: u32 = 256;

fn all_pages<T>(mut read: impl FnMut(PageReq) -> Result<Page<T>, ViewError>) -> Result<Vec<T>> {
    let mut items = Vec::new();
    let mut req = PageReq::first(PAGE);
    loop {
        let page = read(req)?;
        items.extend(page.items);
        match page.next {
            Some(next) => {
                req = PageReq {
                    after: Some(next),
                    limit: PAGE,
                }
            }
            None => return Ok(items),
        }
    }
}

/// Every blob row in `state` (every state when `None`), read from one view.
fn rows_in(backend: &dyn StoreBackend, state: Option<BlobState>) -> Result<Vec<BlobRow>> {
    let view = backend.view()?;
    all_pages(|page| view.blobs(&BlobQuery { hash: None, state }, page))
}

fn blob_row(view: &dyn ReadView, hash: &BlobHash) -> Result<Option<BlobRow>> {
    let q = BlobQuery {
        hash: Some(hash.clone()),
        state: None,
    };
    Ok(view.blobs(&q, PageReq::first(1))?.items.into_iter().next())
}
