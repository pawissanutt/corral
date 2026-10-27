//! Readiness consumes the maintained current-run proof and the last integrity observation.
//! Corral-mediated artifact edits invalidate that observation by revision. Direct filesystem
//! corruption is detected at verification/acceptance, then reflected in subsequent readiness;
//! readback does not rehash snapshots on each board frame.
use anyhow::Result;
use corral_core::accepted_proof::{
    INTEGRITY_CHECKS, IntegrityCheck, ProofFailure, ProofFailureKind,
};
use corral_core::{Artifact, Store, TaskHead};

use crate::Ctx;

const CHECKS: corral_core::records::RecordArea<IntegrityCheck> =
    corral_core::records::RecordArea::new(INTEGRITY_CHECKS);

pub(super) fn readiness(
    store: &Store,
    child: &TaskHead,
    run: &str,
) -> Result<Option<ProofFailure>> {
    let proof = &child.summary.current_artifacts.accepted_proof;
    let checked = CHECKS.load(store, &format!("{}/{run}", child.id))?;
    let failure = checked
        .filter(|checked| checked.revision == proof.revision)
        .and_then(|checked| checked.failure);
    // Registration order, not error kind, decides the original capture's first blocker.
    Ok(proof
        .first_purged
        .iter()
        .chain(failure.as_ref())
        .min_by_key(|failure| failure.ordinal)
        .cloned())
}

/// Real integrity checks retain the original registration order and byte/SHA refusal.
/// Publish both failure and recovery, bound to the exact artifact revision that was checked.
pub(super) fn check(
    ctx: &Ctx,
    store: &Store,
    child: &TaskHead,
    run: &str,
    artifacts: &[Artifact],
) -> Result<Option<ProofFailure>> {
    let failure = artifacts
        .iter()
        .filter(|artifact| artifact.run == run)
        .enumerate()
        .find_map(|(index, artifact)| {
            let kind = if artifact.purged_at.is_some() {
                Some(ProofFailureKind::Purged)
            } else {
                let intact = crate::artifact_storage::artifact_read_confined(ctx, artifact)
                    .is_ok_and(|bytes| {
                        u64::try_from(bytes.len()).ok() == Some(artifact.bytes)
                            && crate::verification::digest(&bytes) == artifact.sha256
                    });
                (!intact).then_some(ProofFailureKind::MissingOrChanged)
            };
            kind.map(|kind| ProofFailure {
                ordinal: u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1),
                path: artifact.path.clone(),
                kind,
            })
        });
    if !store.is_snapshot() {
        let key = format!("{}/{run}", child.id);
        let checked = IntegrityCheck {
            revision: child
                .summary
                .current_artifacts
                .accepted_proof
                .revision
                .clone(),
            failure: failure.clone(),
        };
        // A failed acceptance prepare discards ordinary buffered records. This observation
        // must survive that refusal, just as an intent/effect journal does.
        CHECKS.save_intent(store, &key, &checked)?;
    }
    Ok(failure)
}
