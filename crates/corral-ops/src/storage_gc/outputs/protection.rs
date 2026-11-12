//! Evidence sources are consumers too, including artifacts of another task/session.
//! Root lookup is bounded by declaration depth, never artifact history.
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, ensure};
use corral_core::fs_guard::DirHandle;
use corral_core::records::RecordArea;
use corral_core::{Store, StoreLock};
use serde::{Deserialize, Serialize};

use super::{OWNERS, Owner, key};
use crate::Ctx;

const ROOTS: RecordArea<Root> = RecordArea::new("output-source-roots");
const RESERVATIONS: RecordArea<Root> = RecordArea::new("output-source-reservations");
const PROTECTED: RecordArea<bool> = RecordArea::new("output-source-protection");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Root {
    task: String,
    class: String,
    epoch: String,
}

fn path_key(path: &Path) -> String {
    crate::verification::digest(path.as_os_str().as_encoded_bytes())
}

// Protection belongs to the physical directory, not the producing run. A run transfer
// cannot lose a concurrent evidence intent; inode reuse only retains more, never less.
fn protection_key(path: &Path, dev: u64, ino: u64) -> String {
    format!("{}-{dev}-{ino}", path_key(path))
}

pub(super) fn lease_name(path: &Path) -> String {
    format!("output-root-{}", path_key(path))
}

/// Retention-only intent before an empty-root check/creation. A separate area avoids
/// invalidating the caller's buffered publication CAS; no Owner means no GC authority.
pub(super) fn reserve(
    store: &Store,
    task: &str,
    class: &str,
    path: &Path,
    epoch: &str,
) -> Result<()> {
    RESERVATIONS.save_intent(
        store,
        &path_key(path),
        &Root {
            task: task.into(),
            class: class.into(),
            epoch: epoch.into(),
        },
    )
}

pub(super) fn validate(store: &Store, owner: &Owner, class: &str) -> Result<()> {
    RESERVATIONS.load(store, &path_key(&owner.path))?;
    ensure!(
        ROOTS.load(store, &path_key(&owner.path))?
            == Some(Root {
                task: owner.task.clone(),
                class: class.into(),
                epoch: owner.epoch.clone(),
            }),
        "output source/consumer registration is incomplete"
    );
    Ok(())
}

pub(super) fn protected(store: &Store, owner: &Owner, class: &str) -> Result<bool> {
    validate(store, owner, class)?;
    Ok(PROTECTED
        .load(store, &protection_key(&owner.path, owner.dev, owner.ino))?
        .unwrap_or(false))
}

/// Caller retains the path's exclusive lease through its metadata commit and launch.
pub(super) fn publish(store: &Store, class: &str, owner: &Owner) -> Result<()> {
    store.records_transaction(|| {
        OWNERS.save(store, &key(&owner.task, class), owner)?;
        ROOTS.save(
            store,
            &path_key(&owner.path),
            &Root {
                task: owner.task.clone(),
                class: class.into(),
                epoch: owner.epoch.clone(),
            },
        )
    })
}

/// The shared lease covers artifact reading/publication. Its durable retention intent
/// survives a failed attachment; a source intended as evidence is conservatively kept.
pub(crate) fn protect_artifact_source(ctx: &Ctx, source: &Path) -> Result<Option<StoreLock>> {
    let source =
        std::fs::canonicalize(source).context("resolve artifact source for output ownership")?;
    let parts: Vec<_> = source.components().collect();
    for (at, part) in parts.iter().enumerate() {
        if part.as_os_str() != ".worktrees" {
            continue;
        }
        let Some(task) = parts.get(at + 1).and_then(|p| p.as_os_str().to_str()) else {
            continue;
        };
        if Store::normalize_id(task).ok().as_deref() != Some(task) {
            continue;
        }
        let mut candidate: PathBuf = parts[..at + 2].iter().collect();
        // At most eight declared components; the source leaf cannot be an output root.
        for part in parts
            .iter()
            .skip(at + 2)
            .take(parts.len().saturating_sub(at + 3).min(8))
        {
            ensure!(
                matches!(part, Component::Normal(_)),
                "unknown artifact source path"
            );
            candidate.push(part.as_os_str());
            let published = ROOTS.load(&ctx.store, &path_key(&candidate))?;
            let reserved = RESERVATIONS.load(&ctx.store, &path_key(&candidate))?;
            if published.is_none() && reserved.is_none() {
                continue;
            }
            // Path-scoped rather than class/run-scoped: transfers, renames and provisional
            // registrations must exclude the same consumer even before publication.
            let lease = ctx
                .store
                .try_lock_shared(&lease_name(&candidate))?
                .context("output cleanup/registration is in progress; retry artifact capture")?;
            let identity = DirHandle::open(&candidate)?.identity();
            PROTECTED.save_intent(
                &ctx.store,
                &protection_key(&candidate, identity.dev, identity.ino),
                &true,
            )?;
            return Ok(Some(lease));
        }
    }
    Ok(None)
}
