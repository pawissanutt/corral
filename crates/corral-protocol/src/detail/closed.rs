//! Closed (done or dropped) tasks, paged on request. The board never carries them: their
//! number only grows, so the list is read page by page instead of projected live.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CLOSED_PAGE_MAX: u64 = 50;
/// Byte bound of an opaque `before` anchor.
pub const CLOSED_ANCHOR_MAX: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClosedPage {
    /// Exact count of closed tasks, independent of the requested anchor; None when the
    /// daemon cannot count it without reading closed history.
    pub total: Option<u64>,
    /// Most recently updated first, ties by task number descending; at most
    /// CLOSED_PAGE_MAX rows.
    pub rows: Vec<ClosedTask>,
    /// Opaque exclusive `before` anchor for the next older page, if any.
    pub older: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClosedTask {
    pub id: String,
    pub title: String,
    /// Canonical status: `done` or `dropped`.
    pub status: String,
    /// Last canonical update, RFC 3339: the list's order key, normally the closing change.
    pub updated: String,
    pub project: Option<String>,
    pub plan: Option<String>,
    /// At most 80 Unicode scalar values.
    pub goal: Option<String>,
}
