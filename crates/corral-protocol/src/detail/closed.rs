//! Done or Dropped tasks, paged on request rather than projected live.
use corral_types::Task;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CLOSED_PAGE_MAX: u64 = 50;
/// Byte bound of an opaque `before` anchor.
pub const CLOSED_ANCHOR_MAX: usize = 256;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClosedPage {
    /// Exact count of closed tasks, independent of the requested anchor; None when the
    /// daemon cannot count it without reading closed history.
    pub total: Option<u64>,
    /// Most recently updated first, ties by task number descending; at most
    /// CLOSED_PAGE_MAX rows.
    #[schemars(with = "Vec<crate::schema_types::Task>")]
    pub rows: Vec<Task>,
    /// Opaque exclusive `before` anchor for the next older page, if any.
    pub older: Option<String>,
}
