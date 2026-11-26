//! Observed gate-runner state and bounded views of its attempt log.
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Check;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateAttempt {
    pub id: String,
    pub task: String,
    pub run: String,
    pub sha: String,
    pub round: u32,
    pub started: Timestamp,
    pub heartbeat: Option<Timestamp>,
    pub last_output: Option<Timestamp>,
    pub command: Option<String>,
    pub step: Option<u32>,
    pub steps: u32,
    pub state: GateAttemptState,
    pub finished: Option<Timestamp>,
    pub check: Option<Check>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateAttemptState {
    Running,
    Stale,
    Interrupted,
    Passed,
    Failed,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateAttemptPage {
    pub attempt: GateAttempt,
    /// Byte offsets into the raw log; `text` may be a lossy UTF-8 rendering.
    pub start: u64,
    pub end: u64,
    pub total: u64,
    pub text: String,
}
