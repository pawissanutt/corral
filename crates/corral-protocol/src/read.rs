//! Daemon-computed readouts, transferred as ordered DATA frames.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const READ: &str = "read";
pub const READ_V1: &str = "read.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReadKind {
    PmNext,
    PmBrief,
    AgentPmBrief,
    AgentPmNext,
    BoardReadback,
    ClassicFrame,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadRequest {
    pub kind: ReadKind,
    /// Absolute path of the client's executable.
    pub exe: String,
    /// The client's build identity (what `corral build-identity` prints).
    pub build: String,
}

/// The response to `read`. Exactly `frames` Data frames follow it on `stream`, carrying `bytes` bytes of
/// one UTF-8 JSON document, in order, with nothing else written to the connection in between.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadReply {
    /// RFC 3339 time the document was computed.
    pub as_of: String,
    /// Store change sequence the document was computed at.
    pub seq: u64,
    /// True when an earlier computation was reused.
    pub cached: bool,
    pub stream: u32,
    pub bytes: u64,
    pub frames: u32,
}
