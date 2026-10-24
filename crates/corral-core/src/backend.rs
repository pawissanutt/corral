//! The storage contract (plan T-253 sections 1-3, 5 and 7-8).
//!
//! Every record lives behind [`StoreBackend`]: writes are atomic logical [`Operation`]s, reads go
//! through a [`ReadView`] that observes one committed state, and evidence bytes live in a
//! [`BlobStore`] addressed by content hash. Nothing in this module or its callers names a
//! database mechanism; SQL is confined to `crate::sqlite`. [`fake`] is the in-memory reference
//! implementation and [`conformance`] the one suite every implementation must pass.

pub mod conformance;
mod error;
pub mod fake;
mod operation;
mod query;
mod rows;
mod support;

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use anyhow::Result;
use sha2::{Digest, Sha256};

pub use error::{ApplyError, Conflict, FeedError, ViewError};
pub use operation::*;
pub use query::*;
pub use rows::*;
pub use support::{BackendConfig, Clock, FaultInjector, ManualClock, SystemClock, detail_digest};

use crate::blob_ref::BlobHash;

pub trait StoreBackend: Send + Sync + 'static {
    /// A view of the latest committed state; every read on it observes that state.
    fn view(&self) -> Result<Box<dyn ReadView + '_>, ViewError>;
    /// One serializable unit of work: all of `op` and its change entry, or nothing.
    fn apply(&self, op: Operation) -> Result<Committed, ApplyError>;
    fn check(&self) -> Result<IntegrityReport>;
    /// Copies the database only; blobs are not included (plan section 7, backup).
    fn backup_database(&self, dest: &Path) -> Result<()>;
}

/// One committed state. Reads on a view older than `view_max_age` fail with
/// [`ViewError::Expired`] and return nothing.
pub trait ReadView {
    fn seq(&self) -> ChangeSeq;
    fn board(&self, since: Option<ChangeSeq>) -> Result<BoardDelta, FeedError>;
    fn changes(&self, after: ChangeSeq, limit: u32) -> Result<ChangePage, FeedError>;
    fn head(&self, num: TaskNum) -> Result<Option<TaskHeadDoc>, ViewError>;
    fn heads(&self, q: &HeadQuery, page: PageReq) -> Result<Page<TaskHeadDoc>, ViewError>;
    fn task(&self, num: TaskNum, parts: &TaskParts) -> Result<Option<TaskDoc>, ViewError>;
    fn tasks(&self, q: &TaskQuery, page: PageReq) -> Result<Page<TaskHeader>, ViewError>;
    fn events(&self, q: &EventQuery, page: PageReq) -> Result<Page<StoredEvent>, ViewError>;
    fn conditions(
        &self,
        q: &ConditionQuery,
        page: PageReq,
    ) -> Result<Page<ConditionRun>, ViewError>;
    fn records(&self, q: &RecordQuery, page: PageReq) -> Result<Page<StoredRecord>, ViewError>;
    fn aggregates(&self, names: &[AggregateName]) -> Result<Aggregates, ViewError>;
    fn blobs(&self, q: &BlobQuery, page: PageReq) -> Result<Page<BlobRow>, ViewError>;
    fn storage_rows(&self, q: &StorageQuery, page: PageReq) -> Result<Page<StorageRow>, ViewError>;
}

/// Content-addressed evidence bytes (plan section 7).
pub trait BlobStore: Send + Sync + 'static {
    /// Streams `src` to staging, computing its hash; memory stays bounded.
    fn stage(&self, src: &mut dyn Read) -> Result<Staged>;
    /// Publishes staged content under its hash; idempotent.
    fn put_staged(&self, staged: Staged) -> Result<()>;
    fn get(&self, hash: &BlobHash) -> Result<VerifiedReader>;
    /// Removes the content; idempotent.
    fn delete(&self, hash: &BlobHash) -> Result<()>;
    fn list(&self, after: Option<&BlobHash>, limit: u32) -> Result<Vec<BlobFile>>;
}

#[derive(Debug)]
pub enum StagedData {
    Memory(Vec<u8>),
    File(PathBuf),
}

#[derive(Debug)]
pub struct Staged {
    pub hash: BlobHash,
    pub raw_bytes: u64,
    pub stored_bytes: u64,
    pub codec: String,
    pub data: StagedData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobFile {
    pub hash: BlobHash,
    pub stored_bytes: u64,
    pub modified: jiff::Timestamp,
}

/// Decoded blob content that checks its hash at end of stream: reading to the end of altered
/// or truncated content fails with an `InvalidData` error naming the hash (`HashMismatch`).
pub struct VerifiedReader {
    inner: Box<dyn Read + Send>,
    expected: BlobHash,
    hasher: Sha256,
    verified: bool,
}

impl VerifiedReader {
    pub fn new(inner: Box<dyn Read + Send>, expected: BlobHash) -> Self {
        Self {
            inner,
            expected,
            hasher: Sha256::new(),
            verified: false,
        }
    }

    pub fn hash(&self) -> &BlobHash {
        &self.expected
    }
}

impl Read for VerifiedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.hasher.update(&buf[..n]);
            return Ok(n);
        }
        if !buf.is_empty() && !self.verified {
            let actual: String = self
                .hasher
                .clone()
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            if actual != self.expected.as_str() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("HashMismatch: blob {} read as {actual}", self.expected),
                ));
            }
            self.verified = true;
        }
        Ok(0)
    }
}
