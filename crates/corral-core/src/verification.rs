//! Identities and machine receipts shared by gate, review and acceptance.
use serde::{Deserialize, Serialize};

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandReceipt {
    pub command: String,
    pub exit_code: i32,
    /// Byte range in the attempt's combined log.
    pub log_start: u64,
    pub log_end: u64,
}
