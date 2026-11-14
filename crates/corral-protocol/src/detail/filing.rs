//! Global chat-filing summaries, independent of loaded threads or a live assistant.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const FILINGS_PAGE_MAX: u64 = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilingsPage {
    /// Exact count of chat filings, independent of the requested `before` anchor.
    pub total: u64,
    /// Newest first; at most FILINGS_PAGE_MAX summaries.
    pub rows: Vec<FiledIdea>,
    /// Exclusive `before` anchor for the next older page, if any.
    pub older: Option<u64>,
    /// Exclusive `before` anchor for the next newer page; None selects newest.
    pub newer: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FiledIdea {
    /// Canonical filed owner input, stable across retries and quick-thread rolls.
    pub id: String,
    /// Dense, one-based canonical filing order.
    pub seq: u64,
    pub task: String,
    /// Source assistant session, including ended quick-thread generations.
    pub session: String,
    pub quick: bool,
    /// Recorded device name; absent for pane-typed input.
    pub device: Option<String>,
    /// Canonical task creation time, RFC 3339.
    pub at: String,
    pub title: String,
    /// Current canonical task status.
    pub status: String,
    pub decision: Option<FiledDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FiledDecision {
    pub action: String,
    pub into: Option<String>,
}
