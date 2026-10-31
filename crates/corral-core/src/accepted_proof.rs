//! Maintained accepted-delivery facts for aggregate readiness. Artifact publication and full
//! edits fold these with the current-run artifact scalars; readiness never reads their rows.
//! Bytes are checked at verification/acceptance, not on every readback. Out-of-band corruption
//! becomes visible to readiness after that integrity boundary publishes its revision-bound
//! result. A corral-mediated artifact edit changes the revision and retires that result.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Artifact;

/// Constant-size facts of the current run's accepted artifacts, in registration order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AcceptedProof {
    #[serde(with = "crate::container_facts::fixed")]
    pub count: u64,
    /// Versioned per-ordinal accumulator over every artifact field, including retention and compression.
    pub revision: String,
    pub first_purged: Option<ProofFailure>,
}

impl Default for AcceptedProof {
    fn default() -> Self {
        Self {
            count: 0,
            revision: format!("v2:{:x}", Sha256::digest(b"corral.accepted-proof.v2")),
            first_purged: None,
        }
    }
}

impl AcceptedProof {
    /// Folds one registration at publication time. The Debug encoding covers every artifact
    /// field; this revision is an invalidation token, never a candidate/proof identity.
    pub fn fold(&mut self, artifact: &Artifact) {
        self.count += 1;
        self.toggle(self.count, artifact);
        if artifact.purged_at.is_some() && self.first_purged.is_none() {
            self.first_purged = Some(ProofFailure {
                ordinal: self.count,
                path: artifact.path.clone(),
                kind: ProofFailureKind::Purged,
            });
        }
    }

    /// Replaces one registration without reading other registrations. The caller maintains
    /// first_purged through the ordinal index; count and public ordering are unchanged.
    pub(crate) fn replace(&mut self, ordinal: u64, before: &Artifact, after: &Artifact) {
        self.toggle(ordinal, before);
        self.toggle(ordinal, after);
    }

    fn toggle(&mut self, ordinal: u64, artifact: &Artifact) {
        let mut hash = Sha256::new();
        hash.update(b"corral.accepted-proof.v2/registration");
        hash.update(ordinal.to_be_bytes());
        hash.update(format!("{artifact:?}").as_bytes());
        let leaf = format!("{:x}", hash.finalize());
        // A malformed or old token cannot silently become the canonical v2 empty token.
        // Stored legacy tokens are independently rederived before opening any read view.
        let old = self.revision.strip_prefix("v2:").unwrap_or(&self.revision);
        let value: String = old
            .chars()
            .zip(leaf.chars())
            .map(|(a, b)| match (a.to_digit(16), b.to_digit(16)) {
                (Some(a), Some(b)) => char::from_digit(a ^ b, 16).unwrap_or('?'),
                _ => '?',
            })
            .collect();
        self.revision = format!("v2:{value}");
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofFailure {
    #[serde(with = "crate::container_facts::fixed")]
    pub ordinal: u64,
    pub path: String,
    pub kind: ProofFailureKind,
}

impl ProofFailure {
    pub fn blocker(&self, task: &str) -> String {
        let reason = match self.kind {
            ProofFailureKind::Purged => "was purged",
            ProofFailureKind::MissingOrChanged => "is missing or changed",
        };
        format!("{task}: accepted artifact {} {reason}", self.path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofFailureKind {
    Purged,
    MissingOrChanged,
}

/// The latest full integrity check of exactly one accepted-artifact revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrityCheck {
    pub revision: String,
    pub failure: Option<ProofFailure>,
}

pub const INTEGRITY_CHECKS: &str = "accepted-proof-integrity";

/// Retire observations atomically with row edits, including A -> B -> A restoration and
/// switching back to a pre-registered run. Unrelated run/history writes retain the check.
pub(crate) fn changed_runs(
    before: Option<&crate::backend::HistoryRow>,
    after: Option<&crate::backend::HistoryRow>,
) -> std::collections::BTreeSet<String> {
    use crate::backend::TaskPart;
    let part = after.or(before).map(|row| row.part);
    let field = match part {
        Some(TaskPart::Artifact) => "run",
        Some(TaskPart::Run)
            if before.map(|row| (row.current, row.doc.get("id")))
                != after.map(|row| (row.current, row.doc.get("id"))) =>
        {
            "id"
        }
        _ => return Default::default(),
    };
    before
        .into_iter()
        .chain(after)
        .filter_map(|row| row.doc.get(field)?.as_str().map(str::to_owned))
        .collect()
}
