//! State topics use one acknowledged page in flight, never a notification queue.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Health;
use crate::board::{BoardCursor, BoardDelta};

use crate::assistant::ThreadEntry;
use crate::rows::{
    AgentRow, AssistantRow, BuildRow, InboxRow, PlanRow, SettingsSnapshot, UsageRow,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Health,
    Board,
    Classic,
    Inbox,
    Plans,
    Builds,
    Agents,
    Assistant,
    Usage,
    Settings,
    Thread,
    Session,
    Terminal,
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
    Builds {
        #[serde(flatten)]
        delta: RowsDelta<BuildRow>,
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
        /// Current fleet replacement for capable clients; omission means no current fleet.
        #[serde(default, skip_serializing_if = "fleet_is_empty")]
        fleet: Option<SessionFleet>,
    },
    Terminal {
        scope: String,
        epoch: u64,
        /// Complete bounded replacement window; absent on an unavailable pane.
        snapshot: Option<crate::detail::TerminalSnapshot>,
        cursor: BoardCursor,
        more: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
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

/// Bounded app projection; no child instructions, transcripts, reports or paths.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionChild {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub state: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    pub tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// `approve` or `changes` when a review child records its verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<SessionChildVerdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionChildVerdict {
    Approve,
    Changes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionAgentMessage {
    pub seq: u64,
    /// Epoch milliseconds, as in the host fleet journal.
    pub at: u64,
    pub from: String,
    pub to: String,
    /// At most 2,048 Unicode characters; oldest entries are removed to fit a page.
    pub text: String,
    pub delivery: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionFleet {
    pub children: Vec<SessionChild>,
    /// Newest at most 100 entries, in journal order.
    pub messages: Vec<SessionAgentMessage>,
    /// Includes messages removed by journal retention and page byte bounds.
    pub messages_total: u64,
    pub max_parallel: u32,
}

fn fleet_is_empty(fleet: &Option<SessionFleet>) -> bool {
    fleet.as_ref().is_none_or(|fleet| {
        fleet.children.is_empty() && fleet.messages.is_empty() && fleet.messages_total == 0
    })
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
    /// The optional page-level fleet is a current replacement, not a turn delta.
    LiveFleet {},
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

#[cfg(test)]
mod fleet_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_fleet_and_legacy_pages_round_trip() {
        let old = json!({
            "topic": "session", "scope": "T-1_R1", "epoch": 7, "generation": 1,
            "events": [], "cursor": {"rev": 0, "after": null}, "more": false, "gap": null
        });
        let legacy: SubDelta = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(legacy).unwrap(), old);
        let mut current = old;
        current["fleet"] = json!({
            "children": [{
                "id": "review", "kind": "review", "title": "Review", "state": "done",
                "model": "provider/model", "started_at": 1, "ended_at": 2,
                "activity": "read", "tokens": 42, "cost_usd": 0.01,
                "verdict": "approve", "question": true
            }],
            "messages": [{
                "seq": 101, "at": 2, "from": "parent", "to": "review",
                "text": "Ready?", "delivery": "delivered"
            }],
            "messages_total": 101, "max_parallel": 3
        });
        let page: SubDelta = serde_json::from_value(current.clone()).unwrap();
        assert_eq!(serde_json::to_value(page).unwrap(), current);
        let mut empty = current;
        empty["fleet"] = json!({
            "children": [], "messages": [], "messages_total": 0, "max_parallel": 3
        });
        let page: SubDelta = serde_json::from_value(empty).unwrap();
        assert!(serde_json::to_value(page).unwrap().get("fleet").is_none());
    }
}
