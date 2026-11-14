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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListenerInfo {
    pub port: u16,
    pub origins: Vec<String>,
}
