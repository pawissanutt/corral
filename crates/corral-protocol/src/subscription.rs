//! State topics use one acknowledged page in flight, never a notification queue.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Health;
use corral_types::UsageSummary;

use crate::assistant::ThreadEntry;
use crate::rows::{AgentRow, AssistantRow, PlanRow, ProjectRow, SettingsSnapshot, TaskRow};
use crate::session_state::{SendPolicy, SessionState};

/// Serialized fleet and state budgets within a bounded live page.
pub const LIVE_FLEET_BYTES: usize = 32 * 1024;
pub const LIVE_STATE_BYTES: usize = 12 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Health,
    Tasks,
    Plans,
    Projects,
    Agents,
    /// Shared rows for clients granted `assistant.v1` or `build.v1`.
    Assistant,
    Usage,
    Settings,
    Thread,
    Session,
    Terminal,
    #[serde(other)]
    Other,
}

/// `after = Some(id)` resumes within `rev`; None has consumed that whole revision.
/// The empty starting cursor is `{ rev: 0, after: None }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BoardCursor {
    pub rev: u64,
    pub after: Option<String>,
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
    Tasks {
        #[serde(flatten)]
        delta: RowsDelta<TaskRow>,
    },
    Health {
        epoch: u64,
        /// A health page contains the single current snapshot.
        rows: Vec<Health>,
        left: Vec<String>,
        cursor: BoardCursor,
        more: bool,
    },
    Plans {
        #[serde(flatten)]
        delta: RowsDelta<PlanRow>,
    },
    Projects {
        #[serde(flatten)]
        delta: RowsDelta<ProjectRow>,
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
        #[schemars(with = "RowsDelta<crate::schema_types::UsageSummary>")]
        delta: RowsDelta<UsageSummary>,
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
        /// Replacement for capable clients on the first page, after discontinuity, or when
        /// changed since the last page. Omission means unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Option<SessionState>")]
        state: Option<Box<SessionState>>,
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
    /// Helper children use `helper:<kind>`.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_model: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub turns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<f32>,
    /// Counts of edited paths, of edits outside ownership, and of owned paths.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub files: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub outside: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub owns: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub steerable: bool,
    /// At most 256 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn is_zero<T: Default + PartialEq>(value: &T) -> bool {
    value == &T::default()
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
    /// The replacement rides on the page.
    LiveState {},
    /// The named part changed; reread its detail using the last etag.
    LivePart {
        part: String,
    },
    LiveInput {
        input: String,
        #[serde(flatten)]
        state: SessionLiveInputState,
    },
    /// A non-streamed transcript entry was appended.
    LiveEntry {},
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionLiveInputState {
    Accepted,
    Queued {
        policy: SendPolicy,
        position: u32,
    },
    Started {
        turn: u64,
    },
    Withdrawn,
    Rejected {
        reason: String,
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
