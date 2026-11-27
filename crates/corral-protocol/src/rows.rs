//! Rows for the acknowledged application topics.
use corral_types::{Check, GateAttempt, Run, SessionStatus, Status, Task};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use crate::build::BuildFacts;
use crate::device::DeviceInfo;

/// One task in the open board or a plan. Checks are for its current HEAD and review round.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskRow {
    #[schemars(with = "crate::schema_types::Task")]
    pub task: Task,
    #[schemars(with = "Option<crate::schema_types::Run>")]
    pub run: Option<Run>,
    #[schemars(with = "Option<crate::schema_types::Check>")]
    pub gate: Option<Check>,
    /// Latest observed attempt, independent of the task status and of the retained check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<crate::schema_types::GateAttempt>")]
    pub gate_attempt: Option<GateAttempt>,
    #[schemars(with = "Option<crate::schema_types::Check>")]
    pub review: Option<Check>,
    /// Whether the pipeline appended `ready` for the current review.
    pub ready: bool,
    /// Effective limit for this task, including project overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rounds: Option<u32>,
    /// Ended run eligible for explicit session.relaunch after session loss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_session: Option<String>,
    /// Live reviewer session, even when `run` holds the worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<crate::schema_types::Run>")]
    pub reviewer: Option<Run>,
    /// In `task.depends_on` order, including dependencies that are no longer open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<DependencyStatus>>,
    /// Worker profile dispatch would select without an explicit override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_profile: Option<String>,
    /// Present only for a filed board-plan parent: a summary, never dispatched, whose
    /// state follows its children. Their nonzero counts by status, Done and Dropped included;
    /// empty when it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_children: Option<Vec<StatusCount>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DependencyStatus {
    pub id: String,
    /// Absent when the dependency task is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<crate::schema_types::Status>")]
    pub status: Option<Status>,
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
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectRow {
    pub id: String,
    /// Parsed `[projects.<id>]` as serialized by the engine.
    pub config: Map<String, Value>,
    pub open: u64,
    /// Counts exclude Done and Dropped; an empty vector means no open tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_by_status: Option<Vec<StatusCount>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective: Option<EffectiveProjectSettings>,
    /// Local branch names, most recently committed first; absent if unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_branches: Option<Vec<String>>,
}

/// Project settings after applying workspace defaults, before task-specific overrides.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EffectiveProjectSettings {
    pub path: std::path::PathBuf,
    /// The configured or discovered base; absent when discovery cannot determine one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub gate: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<u64>,
    pub env: std::collections::BTreeMap<String, String>,
    pub auto_dispatch: bool,
    pub auto_land: bool,
    pub max_rounds: u32,
    pub branch_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentRow {
    #[schemars(with = "crate::schema_types::Run")]
    pub run: Run,
    #[schemars(with = "crate::schema_types::SessionStatus")]
    pub status: SessionStatus,
    /// The live Pi host's pause flag, absent when there is no host state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
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
    /// Host question id for the owner prompt shown while Waiting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_id: Option<String>,
    pub rev: u64,
    /// `<provider>/<model>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildFacts>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssistantKind {
    Quick,
    Session,
    Build,
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

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingsSnapshot {
    /// Parsed `corral.toml` as serialized by the engine; empty on a parse error.
    pub config: Map<String, Value>,
    pub listener: Option<ListenerInfo>,
    pub devices: Vec<DeviceInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    /// Content fingerprint of `corral.toml`, including comments and formatting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Built-in engine configuration with no workspace overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built_in_defaults: Option<Map<String, Value>>,
    /// Unconfigured project resolved against the built-in workspace defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_defaults: Option<Map<String, Value>>,
    /// Parse problem in an externally edited file; `config.get` still returns the raw TOML.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<crate::schema_types::ConfigProblem>")]
    pub problem: Option<corral_types::ConfigProblem>,
    /// Daemon filesystem observations for this configuration revision. Absent if not observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_facts: Option<Vec<HarnessFact>>,
    /// Only the workspace Pi file location and its configured child profile selections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi_config: Option<PiConfigFacts>,
    /// Effective profile for new daemon-managed Ask/Chat sessions, not board dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_chat_profile: Option<String>,
    /// Each configured extension in name order; empty on a parse error.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<ExtensionHealth>,
    /// Scheduler health for configured rules in name order.
    #[serde(default)]
    pub automations: Vec<AutomationHealth>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AutomationHealth {
    pub rule: String,
    pub last_at: Option<String>,
    pub outcome: Option<String>,
    pub error: Option<String>,
    pub chat: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExtensionHealth {
    pub name: String,
    /// `tool`, `ask` and/or `react`, for the lines the extension configures.
    pub shapes: Vec<String>,
    pub enabled: bool,
    /// The most recent failure; a later successful event delivery clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Events given up on after repeated delivery failures.
    #[serde(default)]
    pub skipped: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessFact {
    /// Exact key in the effective engine harness configuration.
    pub name: String,
    pub status: HarnessStatus,
    /// Literal command program when identifiable; never includes arguments or assignments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    /// Executable file observed on the daemon host, not a guarantee of what a runner launches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessStatus {
    /// An executable file was found. Does not assert that it can launch or authenticate.
    Installed,
    /// No executable file matched the daemon lookup.
    NotFound,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PiConfigFacts {
    /// Absolute path on the daemon host.
    pub path: String,
    pub status: PiConfigStatus,
    /// Named entries in `[build.profiles]`; does not assert launch eligibility.
    pub child_profiles: Vec<PiChildProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PiConfigStatus {
    /// The child profile projection could be read; not a validation of all Pi settings.
    Read,
    Missing,
    #[serde(other)]
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PiChildProfile {
    pub name: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListenerInfo {
    pub port: u16,
    pub origins: Vec<String>,
    /// The address actually bound by the daemon, including its port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_accepts_old_snapshots_and_round_trips_automation_health() {
        let mut json = serde_json::json!({"config": {}, "devices": []});
        let old: SettingsSnapshot = serde_json::from_value(json.clone()).unwrap();
        assert!(old.automations.is_empty());

        json["automations"] = serde_json::json!([{
            "rule": "nightly",
            "last_at": "2026-01-01T00:00:00Z",
            "outcome": "sent",
            "error": null,
            "chat": "42"
        }]);
        let current: SettingsSnapshot = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(current.automations[0].rule, "nightly");
        assert_eq!(
            serde_json::to_value(current).unwrap()["automations"],
            json["automations"]
        );
    }
}
