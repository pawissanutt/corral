//! Rows for the acknowledged application topics.
use corral_types::{Check, Run, SessionStatus, Status, Task};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::device::DeviceInfo;

/// One open task. Checks match the current HEAD and review round, respectively.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskRow {
    #[schemars(with = "crate::schema_types::Task")]
    pub task: Task,
    #[schemars(with = "Option<crate::schema_types::Run>")]
    pub run: Option<Run>,
    #[schemars(with = "Option<crate::schema_types::Check>")]
    pub gate: Option<Check>,
    #[schemars(with = "Option<crate::schema_types::Check>")]
    pub review: Option<Check>,
    /// Whether the pipeline appended `ready` for the current review.
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlanRow {
    #[schemars(with = "crate::schema_types::Task")]
    pub parent: Task,
    pub child_counts: Vec<StatusCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusCount {
    #[schemars(with = "crate::schema_types::Status")]
    pub status: Status,
    pub count: u64,
}

/// A configured project and the count of tasks that are neither Done nor Dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectRow {
    pub id: String,
    /// Parsed `[projects.<id>]` as serialized by the engine.
    pub config: Map<String, Value>,
    pub open: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentRow {
    #[schemars(with = "crate::schema_types::Run")]
    pub run: Run,
    #[schemars(with = "crate::schema_types::SessionStatus")]
    pub status: SessionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CostBasis {
    Reported,
    Estimated,
    Mixed,
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
    /// Frozen host billing/auth route, not inferred from mutable profile configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingsSnapshot {
    /// Parsed `corral.toml` as serialized by the engine.
    pub config: Map<String, Value>,
    pub listener: Option<ListenerInfo>,
    pub devices: Vec<DeviceInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListenerInfo {
    pub port: u16,
    pub origins: Vec<String>,
}
