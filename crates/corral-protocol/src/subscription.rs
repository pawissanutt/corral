//! State topics use one acknowledged page in flight, never a notification queue.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Health;
use crate::board::{BoardCursor, BoardDelta};

use crate::assistant::ThreadEntry;
use crate::rows::{AgentRow, AssistantRow, InboxRow, PlanRow, SettingsSnapshot, UsageRow};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Health,
    Board,
    Classic,
    Inbox,
    Plans,
    Agents,
    Assistant,
    Usage,
    Settings,
    Thread,
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
    #[serde(default)]
    pub scope: Option<String>,
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
    Inbox {
        #[serde(flatten)]
        delta: RowsDelta<InboxRow>,
    },
    Plans {
        #[serde(flatten)]
        delta: RowsDelta<PlanRow>,
    },
    Agents {
        #[serde(flatten)]
        delta: RowsDelta<AgentRow>,
    },
    Assistant {
        #[serde(flatten)]
        delta: RowsDelta<AssistantRow>,
    },
    Usage {
        #[serde(flatten)]
        delta: RowsDelta<UsageRow>,
    },
    Settings {
        #[serde(flatten)]
        delta: RowsDelta<SettingsSnapshot>,
    },
    Thread {
        scope: String,
        #[serde(flatten)]
        delta: RowsDelta<ThreadEntry>,
        live: Option<LiveText>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubReset {
    pub topic: Topic,
    pub epoch: u64,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubAck {
    pub topic: Topic,
    pub cursor: BoardCursor,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubClose {
    pub topic: Topic,
    #[serde(default)]
    pub scope: Option<String>,
}

/// One bounded page in flight; `left` contains removed row IDs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RowsDelta<T> {
    pub epoch: u64,
    pub rows: Vec<T>,
    pub left: Vec<String>,
    pub cursor: BoardCursor,
    pub more: bool,
}

/// UTF-8 text offset in bytes; acknowledged pages advance the running turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LiveText {
    pub entry: String,
    pub turn: u64,
    pub offset: u64,
    /// At most 32 KiB per page.
    pub text: String,
    pub done: bool,
}
