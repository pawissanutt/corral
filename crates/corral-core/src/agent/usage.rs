//! Durable attribution seam for summary/child/oracle billing. These are accounting references,
//! not permission to launch a child or replace the bound transcript.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSessionKind {
    Explore,
    Oracle,
    Summary,
}

/// Persist before launching billable work. The parent container owns the reference; generation
/// is the initiating generation, not whichever generation later reconciles the response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSession {
    pub generation: u32,
    pub kind: UsageSessionKind,
    pub session_id: String,
    pub session_file: PathBuf,
    /// Fallback for session formats without a model-change entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Host invocation key. Legacy accounting-only references have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    /// Missing/uncertain after launch is never evidence of zero billable spend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AuxiliaryOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuxiliaryOutcome {
    Prepared,
    Running,
    Completed,
    Uncertain,
    NotStarted,
}
