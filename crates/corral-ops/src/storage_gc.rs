//! `corral storage gc` (plan T-253 sections 7, 8 and 10): one report of every storage class with
//! bytes, separating what is reclaimed, what moved into compressed blobs, what lives outside
//! `.corral`, legacy files awaiting `migrate --finalize`, rollback archives and fixed roots, plus
//! every retained entry with its reason. A dry run changes nothing; `--yes` removes clean
//! finished worktrees (branches kept), reclaimable scratch, terminal agent sessions once their
//! transcripts are promoted, legacy build output and unreferenced blobs past the horizon, with
//! one audited event per deletion. Fixed class roots and rollback archives are never deleted.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::Ctx;

mod backup;
mod classes;
mod lifecycle;
mod worktrees;

pub use backup::{BackupReport, StorageCheck, backup_storage, check_storage};
pub use lifecycle::lifecycle_pass;

/// The accounting line an entry's bytes belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Bucket {
    Reclaimed,
    CompressedIntoBlobs,
    MovedOutsideCorral,
    LegacyAwaitingFinalize,
    RollbackArchive,
    FixedRoot,
    Retained,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// A dry run would remove it.
    Reclaimable,
    Reclaimed,
    Retained,
    Deferred,
    Failed,
    /// Counted, never deleted by gc.
    Reported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GcEntry {
    pub class: &'static str,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    pub bytes: u64,
    pub bucket: Bucket,
    pub disposition: Disposition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GcReport {
    pub dry_run: bool,
    pub entries: Vec<GcEntry>,
    /// Bytes per bucket; retained, deferred and failed entries count as `retained`.
    pub totals: BTreeMap<Bucket, u64>,
}

impl GcReport {
    fn push(&mut self, entry: GcEntry) {
        self.entries.push(entry);
    }

    /// An entry removed (or removable in a dry run) into `bucket`.
    fn removable(&self, class: &'static str, target: String, bytes: u64) -> GcEntry {
        GcEntry {
            class,
            target,
            task: None,
            bytes,
            bucket: Bucket::Reclaimed,
            disposition: if self.dry_run {
                Disposition::Reclaimable
            } else {
                Disposition::Reclaimed
            },
            reason: None,
        }
    }

    fn total(&mut self) {
        self.totals.clear();
        for entry in &self.entries {
            let bucket = match entry.disposition {
                Disposition::Retained | Disposition::Deferred | Disposition::Failed => {
                    Bucket::Retained
                }
                _ => entry.bucket,
            };
            *self.totals.entry(bucket).or_default() += entry.bytes;
        }
    }
}

fn entry(
    class: &'static str,
    target: impl Into<String>,
    bytes: u64,
    disposition: Disposition,
    reason: Option<String>,
) -> GcEntry {
    GcEntry {
        class,
        target: target.into(),
        task: None,
        bytes,
        bucket: Bucket::Retained,
        disposition,
        reason,
    }
}

/// Runs `storage gc`: a dry run when `apply` is false. `--yes` holds the `storage-gc` lease,
/// so two runs never overlap, and logs one `storage_gc` summary event.
pub fn storage_gc(ctx: &Ctx, apply: bool) -> Result<GcReport> {
    let _lease = match apply {
        true => Some(
            ctx.store
                .try_lock("storage-gc")?
                .context("another `corral storage gc --yes` is running")?,
        ),
        false => None,
    };
    let now = corral_core::clock::now();
    let mut report = GcReport {
        dry_run: !apply,
        entries: Vec::new(),
        totals: BTreeMap::new(),
    };
    classes::worktrees(ctx, now, &mut report)?;
    classes::scratch(ctx, now, &mut report)?;
    classes::agent_sessions(ctx, &mut report)?;
    classes::legacy(ctx, &mut report)?;
    classes::blobs(ctx, now, &mut report)?;
    classes::roots(ctx, &mut report)?;
    report.total();
    if apply {
        let removed = report
            .entries
            .iter()
            .filter(|e| e.disposition == Disposition::Reclaimed)
            .count();
        ctx.store.log_event(
            None,
            "storage_gc",
            serde_json::json!({ "removed": removed, "totals": report.totals }),
        )?;
    }
    Ok(report)
}
