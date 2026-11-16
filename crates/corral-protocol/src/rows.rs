//! Compact app topic projections. Missing canonical facts remain optional.
use crate::board::CostBasis;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InboxRow {
    pub id: String,
    pub kind: InboxKind,
    pub task: Option<String>,
    pub plan: Option<String>,
    pub project: Option<String>,
    pub title: String,
    pub from: String,
    pub asked_at: Option<String>,
    pub needed_by: Option<String>,
    pub blocking: bool,
    pub state: InboxState,
    pub question_rev: Option<u32>,
    pub answerable: bool,
    pub unanswerable_reason: Option<String>,
    /// Canonical next mover and blocker; absent only for older producers.
    #[serde(default)]
    pub attention: Option<AttentionMove>,
    pub rev: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    WorkerQuestion,
    PmAsk,
    AssistantAsk,
    Prompt,
    PlanAcceptance,
    Review,
    Stuck,
    Escalated,
    OwnerDecision,
    PmAction,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum InboxState {
    Open,
    Answered {
        by: String,
        via: String,
        at: String,
    },
    Cancelled,
    Expired,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttentionMove {
    pub whose_move: AttentionActor,
    pub reason: String,
    pub next_step: String,
    /// A CLI hint, never permission to execute a command on the client.
    pub command: Option<String>,
    pub destination: AttentionDestination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttentionActor {
    Owner,
    Pm,
    Worker,
    External,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttentionDestination {
    Inbox,
    Now,
    Plans,
    Review,
    #[serde(other)]
    Other,
}

/// Counts of open owner moves, not task statuses or loaded board groups.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttentionCounts {
    pub total: u64,
    pub inbox: u64,
    pub now: u64,
    pub plans: u64,
    pub review: u64,
}

impl AttentionCounts {
    pub fn from_rows(rows: &[InboxRow]) -> Self {
        let mut counts = Self::default();
        for row in rows {
            let Some(attention) = &row.attention else {
                continue;
            };
            if row.state != InboxState::Open || attention.whose_move != AttentionActor::Owner {
                continue;
            }
            counts.total += 1;
            counts.inbox += 1;
            counts.now += u64::from(row.task.is_some());
            counts.plans += u64::from(attention.destination == AttentionDestination::Plans);
            counts.review += u64::from(attention.destination == AttentionDestination::Review);
        }
        counts
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttentionSnapshot {
    pub items: Vec<InboxRow>,
    pub counts: AttentionCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanRow {
    pub id: String,
    pub title: String,
    /// Canonical record kind; absent only when the producer does not know it.
    #[serde(default)]
    pub role: Option<PlanRole>,
    /// Published tracker for a design, or approved source design for a tracker.
    #[serde(default)]
    pub linked: Option<String>,
    pub state: PlanState,
    pub revision: Option<String>,
    pub delivery: Option<String>,
    pub members_total: u32,
    pub members_landed: u32,
    pub blockers: Vec<String>,
    pub accepted_at: Option<String>,
    /// Distinct projects of the plan's member tasks, sorted; empty when no member names one.
    #[serde(default)]
    pub projects: Vec<String>,
    pub rev: u64,
}

/// One configured project of the workspace (`Topic::Projects`, `projects.v1`). A complete
/// empty page means the workspace has no project management: no board work, PM off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectRow {
    /// Configuration key; the identity tasks reference in `project`.
    pub id: String,
    /// Workspace-relative path when inside the workspace, otherwise absolute.
    pub path: String,
    /// Tasks that are neither done nor dropped.
    pub open: u32,
    /// Open tasks with a live worker session.
    pub running: u32,
    /// Open tasks awaiting review.
    pub review: u32,
    /// Open owner-attention items for this project's tasks.
    pub needs_me: u32,
    /// Whether the workspace PM's approved scope includes this project.
    pub in_pm_scope: bool,
    /// Whether an active autopilot grant names this project.
    pub autopilot: bool,
    pub rev: u64,
}

/// A published build record, independent of task scheduling and statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BuildRow {
    pub id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub title: String,
    pub state: String,
    pub created: String,
    pub updated: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub landed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_reason: Option<String>,
    pub slices: u64,
    pub review_rounds: u64,
    pub gate_attempts: u64,
    pub gate_failures: u64,
    pub messages: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub tokens: u64,
    pub rev: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanRole {
    Design,
    Tracker,
    /// A known ordinary task, not an unknown record kind.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanState {
    /// No submitted revision is waiting for the owner; planning or changes remain.
    Drafting,
    /// A submitted document still needs independent review or automatic processing.
    Reviewing,
    /// Planning or its review failed or escalated, not an acceptance request.
    PlanFailed,
    /// A submitted, unapproved document is waiting for the owner's decision.
    Awaiting,
    Executing,
    /// The document is accepted; a design may already have published its tracker.
    Approved,
    Complete,
    Cancelled,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentRow {
    pub id: String,
    pub role: String,
    pub task: Option<String>,
    pub run: Option<String>,
    pub title: String,
    pub profile: Option<String>,
    pub runtime: Option<String>,
    pub machine: Option<String>,
    pub pane: Option<String>,
    pub status: AgentStatus,
    pub now: String,
    pub since: Option<String>,
    pub tail: Vec<String>,
    pub cost_usd: Option<f64>,
    pub cost_basis: Option<CostBasis>,
    pub tokens: Option<u64>,
    pub rev: u64,
    /// The host generation the controls were computed for; a control command echoes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u32>,
    /// The open turn, when the host reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    /// What the owner may do to this session now. The client never infers these.
    #[serde(default)]
    pub controls: SessionControls,
}

/// One control of a session: offered, or refused with a machine code such as `host_lost`,
/// `no_bridge`, `ended`, `not_owned`, `adopted_pane`, `not_paused`, `no_turn`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ControlState {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

/// Per-session controls, computed by the daemon. A default value offers nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionControls {
    /// Owner words into the session's chat.
    pub send: ControlState,
    /// Cancel the current turn.
    pub cancel: ControlState,
    pub pause: ControlState,
    pub resume: ControlState,
    /// Terminate: stop the host and end the session.
    pub stop: ControlState,
    /// Start the next generation of a lost host (for the PM row: open the PM).
    pub relaunch: ControlState,
    /// Keys typed into the session's pane.
    pub keys: ControlState,
    /// Move a pane-hosted assistant into the daemon.
    pub take_over: ControlState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Live,
    Working,
    Asking,
    Gating,
    Reviewing,
    Idle,
    Waiting,
    /// Fenced by the owner: alive, taking no automated input until resumed.
    Paused,
    Ended,
    Unavailable,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AssistantRow {
    pub id: String,
    pub kind: AssistantKind,
    pub title: String,
    pub state: AssistantState,
    pub live_in: AssistantLocation,
    pub profile: Option<String>,
    pub focus: Option<String>,
    pub pending: u32,
    pub turn: Option<u64>,
    pub cost_usd: Option<f64>,
    pub cost_basis: Option<CostBasis>,
    pub wait: Option<WaitReason>,
    pub rev: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssistantKind {
    Quick,
    Session,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssistantState {
    Cold,
    Starting,
    Ready,
    Working,
    Waiting,
    Stopping,
    /// Fenced by the owner: alive, queued messages wait until resumed.
    Paused,
    Ended,
    Failed,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssistantLocation {
    Pane,
    Daemon,
    None,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WaitReason {
    pub code: String,
    pub text: String,
    pub retry_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageRow {
    pub id: String,
    pub connection: String,
    pub profile: Option<String>,
    pub window: Option<String>,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub limit_basis: LimitBasis,
    pub resets_at: Option<String>,
    pub today_usd: Option<f64>,
    pub week_usd: Option<f64>,
    pub cost_basis: Option<CostBasis>,
    #[serde(default)]
    pub today_completeness: crate::detail::UsageCompleteness,
    #[serde(default)]
    pub week_completeness: crate::detail::UsageCompleteness,
    #[serde(default)]
    pub quotas: Vec<crate::detail::UsageQuota>,
    pub rev: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LimitBasis {
    Declared,
    Observed,
    None,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingsSnapshot {
    pub id: String,
    pub workspace: String,
    pub store: String,
    pub host: String,
    pub mode: OperatingMode,
    pub grants: Vec<GrantLine>,
    pub assistant_models: Vec<ModelChoice>,
    /// Exact configured profiles, separate from assistant-compatible model choices.
    #[serde(default)]
    pub profiles: Vec<ProfileChoice>,
    pub assistant_profile: Option<String>,
    pub summary_profile: Option<String>,
    pub listener: Option<ListenerInfo>,
    pub config_hash: String,
    pub rev: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperatingMode {
    Assist,
    Autopilot,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GrantLine {
    pub key: String,
    pub enabled: bool,
    pub scope: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelChoice {
    pub name: String,
    pub model: String,
    pub effort: Option<String>,
    pub note: Option<String>,
}

/// A configured profile's safe editable values and canonical removal restrictions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileChoice {
    pub name: String,
    pub harness: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// A workspace entry exists, including partial overrides of inherited profiles.
    pub workspace: bool,
    /// Removing the workspace entry would leave a personal or built-in profile.
    pub inherited: bool,
    /// Current configured/task/session references, for explanation rather than authority.
    pub references: Vec<String>,
    /// None only when canonical removal is currently possible; rechecked at commit.
    pub remove_refusal: Option<String>,
    /// A configured profile may be unsuitable for an independent quick-thread summary request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_refusal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListenerInfo {
    pub port: u16,
    pub origins: Vec<String>,
}
