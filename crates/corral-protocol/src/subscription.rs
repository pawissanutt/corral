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
    Session,
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
    Session {
        scope: String,
        epoch: u64,
        generation: Option<u32>,
        events: Vec<SessionLiveEvent>,
        cursor: BoardCursor,
        more: bool,
        gap: Option<SessionLiveGap>,
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

/// Text/thinking offsets are UTF-8 bytes, independent per (generation, turn, message,
/// kind). Applying chunks at their offsets, rather than appending, is idempotent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionLiveEvent {
    pub seq: u64,
    pub generation: u32,
    pub at: String,
    #[serde(flatten)]
    pub kind: SessionLiveKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionLiveKind {
    LiveTurn {
        turn: u64,
        #[serde(flatten)]
        state: SessionLiveTurnState,
    },
    LiveText {
        turn: u64,
        message: u64,
        offset: u64,
        text: String,
    },
    LiveThinking {
        turn: u64,
        message: u64,
        offset: u64,
        text: String,
    },
    LiveTool {
        turn: u64,
        call: String,
        tool: String,
        #[serde(flatten)]
        state: SessionLiveToolState,
    },
    LiveQueue {
        steer: u64,
        follow_up: u64,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionLiveTurnState {
    Started,
    Ended {
        aborted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionLiveToolState {
    Started {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args_truncated: Option<bool>,
    },
    Updated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        partial: Option<String>,
    },
    Ended {
        failed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elapsed_ms: Option<u64>,
    },
    #[serde(other)]
    Other,
}

/// Drop the live overlay and reconcile from `detail.get { session }`. A zero
/// `missing` still signals discontinuity (generation change or unavailable host).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionLiveGap {
    pub after: u64,
    pub resumed: u64,
    pub missing: u64,
    pub reconcile: bool,
}
