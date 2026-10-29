//! `--finalize --yes`: `published` -> `finalizing` (with the legacy paths it will delete) under
//! the exclusive gate, then the deletion under the `storage-gc` claim, then `finalized`.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::journal::{Journal, JournalState};
use super::scan::walk;
use super::{FENCE_PATH, MANIFEST, Migrate, durable, locks};
use crate::backend::{
    ApplyError, Claim, ClaimRow, Operation, PageReq, ReleaseClaim, STORAGE_GC_CLAIM, StorageQuery,
    StorageRow, StoreBackend,
};
use crate::blob::{ProcessIdentity, proven_dead};

#[derive(Debug, Clone, Serialize)]
pub struct Finalized {
    pub state: String,
    pub deleted: Vec<String>,
}

/// The legacy paths finalize deletes: each top-level legacy entry, and each `state/` and
/// `prompts/` child, so the fence and the owner's prompt overrides stay.
fn legacy_paths(corral: &std::path::Path) -> Result<Vec<String>> {
    let manifest = walk(corral)?;
    let mut paths = BTreeSet::new();
    let fence_dir = FENCE_PATH.split('/').next().unwrap_or("state");
    let per_child = [fence_dir, "prompts"];
    for path in manifest
        .entries
        .iter()
        .map(|e| e.path.as_str())
        .chain(manifest.build_roots.iter().map(|r| r.path.as_str()))
    {
        let mut parts = path.splitn(3, '/');
        let first = parts.next().unwrap_or(path);
        match parts.next() {
            Some(second) if per_child.contains(&first) => {
                paths.insert(format!("{first}/{second}"));
            }
            _ => {
                paths.insert(first.to_owned());
            }
        }
    }
    Ok(paths.into_iter().collect())
}

pub(super) fn finalize(m: &Migrate) -> Result<Finalized> {
    let corral = m.corral_dir();
    let _migrate = locks::migrate_lock(corral, m.wait)?;
    let Some(mut journal) = Journal::read(corral)? else {
        bail!(
            "nothing to finalize: {} has no migration journal",
            corral.display()
        );
    };
    match journal.state {
        JournalState::Finalized => {
            return Ok(Finalized {
                state: journal.state.as_str().to_owned(),
                deleted: Vec::new(),
            });
        }
        JournalState::Published => {
            let gate = locks::store_gate(corral, m.wait, || m.hit("gate_busy"))?;
            journal.legacy_paths = legacy_paths(corral)?;
            journal.state = JournalState::Finalizing;
            journal.write(corral)?;
            drop(gate);
            m.hit("B18")?;
        }
        JournalState::Finalizing => {}
        JournalState::RollingBack => bail!("finish the rollback first (corral migrate --rollback)"),
        other => bail!(
            "finalize requires a published migration; the journal is {}",
            other.as_str()
        ),
    }
    let root = corral
        .parent()
        .context("the store directory has no parent")?;
    let store = crate::Store::open_unobserved(root)?;
    let backend = store.backend();
    let me = ProcessIdentity::current()?;
    claim(backend, &me)?;
    let deleted = delete(m, &journal.legacy_paths);
    let released = backend
        .apply(Operation::ReleaseClaim(ReleaseClaim {
            name: STORAGE_GC_CLAIM.to_owned(),
            owner: me.owner(),
        }))
        .map_err(|error| anyhow::anyhow!("releasing the storage-gc claim: {error}"));
    let deleted = deleted?;
    released?;
    durable::unlink(&corral.join(MANIFEST))?;
    journal.state = JournalState::Finalized;
    journal.write(corral)?;
    Ok(Finalized {
        state: journal.state.as_str().to_owned(),
        deleted,
    })
}

/// Takes the `storage-gc` claim, first releasing it from a holder proven dead (a finalize or
/// storage gc killed while holding it); a live or unrecognized holder keeps it.
fn claim(backend: &dyn StoreBackend, me: &ProcessIdentity) -> Result<()> {
    if let Some(held) = gc_claim(backend)? {
        let dead = match serde_json::from_value::<ProcessIdentity>(held.doc.clone()) {
            Ok(holder) if holder.owner() == held.owner => proven_dead(&holder)?,
            _ => false,
        };
        if !dead {
            bail!(
                "{} holds the storage-gc claim; rerun corral migrate --finalize --yes after it exits",
                held.owner
            );
        }
        let release = Operation::ReleaseClaim(ReleaseClaim {
            name: STORAGE_GC_CLAIM.to_owned(),
            owner: held.owner.clone(),
        });
        match backend.apply(release) {
            Ok(_) | Err(ApplyError::Precondition(_)) => {}
            Err(error) => bail!(
                "releasing the storage-gc claim of dead holder {}: {error}",
                held.owner
            ),
        }
    }
    let op = Operation::Claim(Claim {
        name: STORAGE_GC_CLAIM.to_owned(),
        owner: me.owner(),
        acquired: jiff::Timestamp::now(),
        doc: serde_json::to_value(me)?,
        reservations: Vec::new(),
    });
    match backend.apply(op) {
        Ok(_) => Ok(()),
        Err(ApplyError::Conflict(conflict)) => bail!(
            "storage gc holds the storage-gc claim ({conflict:?}); rerun corral migrate --finalize --yes after it exits"
        ),
        Err(error) => Err(anyhow::anyhow!("taking the storage-gc claim: {error}")),
    }
}

fn gc_claim(backend: &dyn StoreBackend) -> Result<Option<ClaimRow>> {
    let view = backend.view()?;
    let mut req = PageReq::first(256);
    loop {
        let page = view.storage_rows(&StorageQuery::Claims, req)?;
        for row in page.items {
            if let StorageRow::Claim(claim) = row
                && claim.name == STORAGE_GC_CLAIM
            {
                return Ok(Some(claim));
            }
        }
        match page.next {
            Some(next) => {
                req = PageReq {
                    after: Some(next),
                    limit: 256,
                }
            }
            None => return Ok(None),
        }
    }
}

fn delete(m: &Migrate, paths: &[String]) -> Result<Vec<String>> {
    let corral = m.corral_dir();
    let mut deleted = Vec::new();
    for (i, rel) in paths.iter().enumerate() {
        let path = corral.join(rel);
        match durable::seen(&path)? {
            durable::Seen::Absent => continue,
            durable::Seen::Dir => durable::remove_tree(&path, || Ok(()))?,
            _ => durable::unlink(&path)?,
        }
        deleted.push(rel.clone());
        if i == 0 {
            m.hit("B19")?;
        }
    }
    Ok(deleted)
}
