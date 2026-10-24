//! The blob half of `storage check` (plan T-253 section 7): every live row's content must
//! decode to its hash.

use std::fmt;

use anyhow::Result;
use serde::Serialize;

use super::rows_in;
use crate::backend::{BlobState, BlobStore, StoreBackend};
use crate::blob_ref::BlobHash;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum BlobProblem {
    /// The content of a live row cannot be opened.
    Missing { hash: BlobHash, detail: String },
    /// The content of a live row was altered or truncated.
    HashMismatch { hash: BlobHash, detail: String },
}

impl BlobProblem {
    pub fn hash(&self) -> &BlobHash {
        match self {
            Self::Missing { hash, .. } | Self::HashMismatch { hash, .. } => hash,
        }
    }
}

impl fmt::Display for BlobProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { hash, detail } => write!(f, "blob {hash} is missing: {detail}"),
            Self::HashMismatch { hash, detail } => {
                write!(f, "blob {hash} fails verification: {detail}")
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BlobCheck {
    pub checked: u64,
    pub problems: Vec<BlobProblem>,
}

impl BlobCheck {
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Reads every live blob to its end through `blobs`, streaming, and reports each one whose
/// content is missing or does not match its hash.
pub fn check_blobs(backend: &dyn StoreBackend, blobs: &dyn BlobStore) -> Result<BlobCheck> {
    let mut report = BlobCheck::default();
    for row in rows_in(backend, Some(BlobState::Live))? {
        report.checked += 1;
        let mut reader = match blobs.get(&row.hash) {
            Ok(reader) => reader,
            Err(error) => {
                report.problems.push(BlobProblem::Missing {
                    hash: row.hash,
                    detail: format!("{error:#}"),
                });
                continue;
            }
        };
        if let Err(error) = std::io::copy(&mut reader, &mut std::io::sink()) {
            report.problems.push(BlobProblem::HashMismatch {
                hash: row.hash,
                detail: error.to_string(),
            });
        }
    }
    Ok(report)
}
