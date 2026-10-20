//! Identities and machine receipts shared by gate, review and acceptance.
use serde::{Deserialize, Serialize};

/// Inputs exported by gate scripts and compared by environment diagnostics.
pub const VERIFICATION_ENV: [&str; 9] = [
    "PATH",
    "RUSTUP_TOOLCHAIN",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTFLAGS",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "CARGO_TARGET_DIR",
    "CORRAL_HERDR_SESSION",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateIdentity {
    pub version: u32,
    pub task: String,
    pub run: String,
    pub attempt: String,
    pub head: String,
    pub tree: String,
    pub integration_base: String,
    pub dispatch_base: String,
    pub spec_digest: String,
    pub policy_digest: String,
    pub inputs_digest: String,
    /// Selected gate environment, captured once and replayed by every verifier.
    /// Empty on legacy receipts, which cannot authorize a version 2 candidate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdentityChange {
    pub field: String,
    pub expected: String,
    pub actual: String,
}

impl CandidateIdentity {
    /// Structured differences for stale-receipt diagnostics, in stable field order.
    pub fn differences(&self, actual: &Self) -> Vec<IdentityChange> {
        let mut changes = Vec::new();
        if self.version != actual.version {
            changes.push(IdentityChange {
                field: "version".into(),
                expected: self.version.to_string(),
                actual: actual.version.to_string(),
            });
        }
        for (field, expected, observed) in [
            ("task", &self.task, &actual.task),
            ("run", &self.run, &actual.run),
            ("attempt", &self.attempt, &actual.attempt),
            ("head", &self.head, &actual.head),
            ("tree", &self.tree, &actual.tree),
            (
                "integration_base",
                &self.integration_base,
                &actual.integration_base,
            ),
            ("dispatch_base", &self.dispatch_base, &actual.dispatch_base),
            ("spec_digest", &self.spec_digest, &actual.spec_digest),
            ("policy_digest", &self.policy_digest, &actual.policy_digest),
            ("inputs_digest", &self.inputs_digest, &actual.inputs_digest),
        ] {
            if expected != observed {
                changes.push(IdentityChange {
                    field: field.into(),
                    expected: expected.clone(),
                    actual: observed.clone(),
                });
            }
        }
        changes
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandReceipt {
    pub command: String,
    pub exit_code: i32,
    /// Byte range in the attempt's combined log.
    pub log_start: u64,
    pub log_end: u64,
    /// Command wall-clock bounds; absent on receipts written before they were recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<jiff::Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<jiff::Timestamp>,
}
