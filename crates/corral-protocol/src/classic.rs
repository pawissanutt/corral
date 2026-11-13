//! Change notices for the daemon-computed classic board frame; the frame itself is a served read.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CLASSIC_V1: &str = "classic.v1";

/// Pushed on topic `classic` when the daemon's frame revision passes the subscriber's cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClassicNotice {
    /// This daemon process's frame series; any other value means resubscribe.
    pub epoch: u64,
    /// Strictly increasing within `epoch`; bumps only when frame content changed.
    pub rev: u64,
    /// Store change sequence the frame was checked through.
    pub seq: u64,
    /// RFC 3339 time of the recompute that produced `rev`.
    pub as_of: String,
}
