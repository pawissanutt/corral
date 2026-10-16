//! Explicit owner authorization for recurring report tasks.
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// A schedule exists only after explicit automatic-dispatch authorization.
/// Each due occurrence queues at most one run; missed intervals are coalesced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardSchedule {
    pub interval_secs: u32,
    pub next_due: Timestamp,
    pub paused: bool,
}
