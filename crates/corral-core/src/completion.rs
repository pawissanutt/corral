//! Explicit owner-reviewed criteria and append-only worker observations.
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::CandidateIdentity;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Criterion {
    pub id: String,
    pub required: bool,
    /// Require an actual live capture, not an offline assertion.
    pub live: bool,
    pub scenario: String,
    pub expected: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CriteriaManifest {
    pub version: u32,
    pub spec_digest: String,
    pub reviewed_by: String,
    /// Explicit opt-in; absent manifests preserve legacy completion.
    pub audited: bool,
    /// Explicit no-gate policy is permitted only without configured commands.
    pub require_gate: bool,
    pub criteria: Vec<Criterion>,
    #[serde(default)]
    pub owner_decisions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStatus {
    Pass,
    Fail,
    NotRun,
}

/// Input is an assertion, never a corral-executed PASS receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceObservation {
    pub version: u32,
    pub key: String,
    pub criterion: String,
    pub run: String,
    pub attempt: String,
    pub status: ObservationStatus,
    pub action: String,
    pub environment: String,
    pub expected: String,
    pub observed: String,
    pub artifacts: Vec<String>,
    pub captured: Timestamp,
    pub producer: String,
    pub live: bool,
    #[serde(default)]
    pub cleanup: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub source: EvidenceSource,
    pub observation: EvidenceObservation,
    /// Computed by corral at registration, never copied from worker input.
    pub identity: CandidateIdentity,
    pub manifest_digest: String,
    pub artifact_hashes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    WorkerAssertion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionAudit {
    pub version: u32,
    pub audited: bool,
    pub passed: bool,
    pub identity: Option<CandidateIdentity>,
    pub manifest_digest: Option<String>,
    pub required_checks: Vec<String>,
    pub evidence_keys: Vec<String>,
    /// Stable machine-readable condition names, optionally followed by an ID.
    pub failures: Vec<String>,
    pub checked: Timestamp,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_checks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<CriteriaManifest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit: Option<CompletionAudit>,
    /// Latest authorized carry of the records bound to an earlier identity; beside the ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_carry: Option<Box<crate::EvidenceCarry>>,
}
