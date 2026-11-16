//! Device-bound app sessions and bounded CONTROL-only operations.
use crate::{cmd, device::DeviceInfo};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const APP_SESSION: &str = "app.session";
pub const ACT_PREVIEW: &str = "act.preview";
pub const ACT_SUBMIT: &str = "act.submit";
pub const ACT_RESULT: &str = "act.result";
pub const DETAIL_GET: &str = "detail.get";
pub const CHANGES_PAGE: &str = "changes.page";
pub const SEEN_ADVANCE: &str = "seen.advance";
pub const DEVICE_SET_AUTHORITY: &str = "device.set_authority";
pub const INBOX_V1: &str = "inbox.v1";
pub const CHANGES_V1: &str = "changes.v1";
pub const PLANS_V1: &str = "plans.v1";
pub const BUILDS_V1: &str = "builds.v1";
/// Configured workspace projects (`Topic::Projects`) and `PlanRow.projects`.
pub const PROJECTS_V1: &str = "projects.v1";
pub const AGENTS_V1: &str = "agents.v1";
pub const SESSION_V1: &str = "session.v1";
pub const SESSION_FLEET_V1: &str = "session.fleet.v1";
pub const REVIEW_V1: &str = "review.v1";
pub const TIMELINE_V1: &str = "timeline.v1";
pub const USAGE_V1: &str = "usage.v1";
pub const SETTINGS_V1: &str = "settings.v1";
pub const ASSISTANT_V1: &str = "assistant.v1";
pub const FILINGS_V1: &str = "filings.v1";
pub const CLOSED_V1: &str = "closed.v1";
/// `AgentRow.last_output_at`, `activity` and `steps`.
pub const AGENTS_ACTIVITY_V1: &str = "agents.activity.v1";
pub const ACT_V1: &str = "act.v1";
/// Includes the complete JSON-RPC envelope, not only its result.
pub const APP_CONTROL_MAX: usize = 60 * 1024;
pub const CHANGES_LIMIT_MAX: u16 = 200;
pub const PREVIEW_TTL_SECS: u64 = 60;
pub const DEVICE_COMMAND_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppSessionRequest {}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceAuthority {
    pub reply: bool,
    pub act: bool,
    pub assistant: bool,
    pub assistant_act: bool,
    pub admin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceSetAuthority {
    pub device: String,
    pub authority: DeviceAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppSession {
    pub device: DeviceInfo,
    pub authority: DeviceAuthority,
    pub seen: SeenCursor,
    pub server_time: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActClass {
    Reply,
    Act,
    Confirm,
    Admin,
    Refused,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActPreviewRequest {
    pub command: cmd::Command,
    pub expect: Option<cmd::Expect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActPreview {
    pub preview_id: String,
    pub digest: String,
    pub class: ActClass,
    /// At most 16 effects, each at most 256 bytes.
    pub effects: Vec<String>,
    pub expires_at: String,
    /// Canonical operation preview token, when the selected command requires one.
    #[serde(default)]
    pub operation_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfirmProof {
    pub preview_id: String,
    pub digest: String,
}

/// Caller identity and executable admission fields are deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActSubmit {
    pub cmd_id: String,
    pub expect: Option<cmd::Expect>,
    pub issued_at: String,
    pub expires_at: String,
    pub command: cmd::Command,
    pub confirm: Option<ConfirmProof>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChangesQuery {
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeLine {
    pub seq: u64,
    pub at: String,
    pub class: String,
    #[serde(default)]
    pub kind: ChangeKind,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub failed: bool,
    pub task: Option<String>,
    pub text: String,
    pub actor: String,
    pub via: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChangesPage {
    pub lines: Vec<ChangeLine>,
    pub head: u64,
    pub older: Option<u64>,
    pub seen: SeenCursor,
    /// Exact owner-relevant totals, independent of page size and history cursor.
    /// Unknown only when talking to an older daemon.
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub unseen: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Created,
    Moved,
    Work,
    Gate,
    Review,
    Land,
    Attention,
    Question,
    Comment,
    Annotation,
    Automation,
    Policy,
    Queue,
    Plan,
    #[default]
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SeenAdvance {
    pub through: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SeenCursor {
    pub through: u64,
    pub at: Option<String>,
}
