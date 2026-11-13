//! State topics use one acknowledged page in flight, never a notification queue.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Health;
use crate::board::{BoardCursor, BoardDelta};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Health,
    Board,
    Classic,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubSince {
    pub epoch: u64,
    pub cursor: BoardCursor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubOpen {
    pub topic: Topic,
    pub since: Option<SubSince>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "topic", rename_all = "snake_case")]
pub enum SubDelta {
    Board {
        #[serde(flatten)]
        delta: BoardDelta,
    },
    Health {
        epoch: u64,
        /// A health page contains the single current snapshot.
        rows: Vec<Health>,
        left: Vec<String>,
        cursor: BoardCursor,
        more: bool,
    },
    Classic {
        #[serde(flatten)]
        notice: crate::classic::ClassicNotice,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubReset {
    pub topic: Topic,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubAck {
    pub topic: Topic,
    pub cursor: BoardCursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubClose {
    pub topic: Topic,
}
