//! Bounded replacement state for a live Pi session.
use crate::detail::session_parts::CardKind;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionState {
    pub mode: SessionMode,
    pub turn: u64,
    pub open: bool,
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<Activity>,
    /// At most 512 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// `<provider>/<model>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effort_levels: Vec<String>,
    pub usage: UsageBrief,
    /// First 12 in host order. `queue_total` includes omitted items.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queue: Vec<QueuedInput>,
    pub queue_total: u32,
    /// At most eight open cards.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cards: Vec<CardBrief>,
    /// Build only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<ExitBrief>,
    pub children: Counts,
    pub jobs: Counts,
    /// Changed files in the default comparison; absent until read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<BoardBrief>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanBrief>,
    /// At most four notices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<HostNotice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub can: Vec<Capability>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// A terminal is attached.
    Interactive,
    Headless,
    #[default]
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Activity {
    /// At most 96 bytes.
    pub phase: String,
    pub since: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageBrief {
    pub tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub cost_partial: bool,
    pub context_tokens: u64,
    pub context_window: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QueuedInput {
    pub key: String,
    pub policy: SendPolicy,
    pub origin: InputOrigin,
    /// UTF-8 prefix of at most 200 bytes.
    pub text: String,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SendPolicy {
    Steer,
    FollowUp,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputOrigin {
    Owner,
    Auto,
    Pane,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CardBrief {
    pub id: String,
    pub kind: CardKind,
    /// At most 160 bytes.
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExitBrief {
    pub phase: ExitPhase,
    /// At most 512 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<ExitChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<Repair>,
    /// The current round of Land, from its first step until the next exit choice opens.
    /// Absent from a host that does not report it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub land: Option<LandBrief>,
}

/// One round of Land as its host runs it. A repair starts a new round.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LandBrief {
    /// In the order they run: rebase, gate (absent when the session has no gate), land.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<LandStep>,
    /// The gate this round runs or last ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<LandGate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LandStep {
    pub id: LandStepId,
    pub state: LandStepState,
    /// A short fact: the rebased head, the gate attempt, what stopped it. At most 160 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LandStepId {
    Rebase,
    Gate,
    Land,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LandStepState {
    Waiting,
    Running,
    Done,
    Failed,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LandGate {
    /// `G3`, once the attempt is recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    /// Absolute path of the gate's log on the host. At most 512 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
    /// The command running, or the one that failed. At most 240 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// 1-based place of `command` among the gate's `of` commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<u32>,
    /// The log's last lines, oldest first, without terminal sequences: at most 8 lines of at
    /// most 200 bytes. Empty once the gate passed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tail: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExitPhase {
    None,
    Pending,
    Rebasing,
    Verifying,
    Repairing,
    Landing,
    Kept,
    Landed,
    Failed,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExitChoice {
    Keep,
    Land,
    Iterate,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Repair {
    pub round: u32,
    pub of: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Counts {
    pub live: u32,
    pub waiting: u32,
    pub total: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BoardBrief {
    pub open: u32,
    pub blocked: u32,
    pub failed: u32,
    pub proposed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanBrief {
    pub done: u32,
    pub total: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostNotice {
    pub id: String,
    pub tone: String,
    /// At most 240 bytes.
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
}

/// An absent ID means unsupported for this role. A reason means supported but unavailable now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Capability {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::{BoardCursor, SessionLiveEvent, SessionLiveGap, SubDelta, Topic};
    use serde_json::json;

    // This reader intentionally does not know about the state field.
    #[derive(Debug, PartialEq, Deserialize)]
    struct LegacyPage {
        topic: Topic,
        scope: String,
        epoch: u64,
        generation: Option<u32>,
        events: Vec<SessionLiveEvent>,
        cursor: BoardCursor,
        more: bool,
        gap: Option<SessionLiveGap>,
    }

    #[test]
    fn a_page_without_state_reads_as_unchanged_for_old_and_new_clients() {
        let old = json!({
            "topic": "session", "scope": "A-1", "epoch": 7, "generation": 2,
            "events": [], "cursor": {"rev": 18, "after": null}, "more": false, "gap": null
        });
        let mut page: SubDelta = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(&page).unwrap(), old);
        let replacement = SessionState {
            mode: SessionMode::Headless,
            turn: 3,
            open: true,
            ..SessionState::default()
        };
        let SubDelta::Session { state, .. } = &mut page else {
            panic!("expected a live session page");
        };
        assert_eq!(*state, None);
        *state = Some(Box::new(replacement.clone()));
        let current = serde_json::to_value(&page).unwrap();
        assert_eq!(
            serde_json::from_value::<LegacyPage>(old).unwrap(),
            serde_json::from_value::<LegacyPage>(current.clone()).unwrap(),
        );
        let SubDelta::Session { state, .. } = serde_json::from_value(current).unwrap() else {
            panic!("expected a live session page");
        };
        assert_eq!(state, Some(Box::new(replacement)));
    }
}
