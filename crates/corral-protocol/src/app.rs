//! Device-bound app sessions and bounded CONTROL-only operations.
use corral_types::Event;

use crate::device::DeviceInfo;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const APP_SESSION: &str = "app.session";
pub const DETAIL_GET: &str = "detail.get";
pub const CHANGES_PAGE: &str = "changes.page";
pub const SEEN_ADVANCE: &str = "seen.advance";
pub const CHANGES_V1: &str = "changes.v1";
pub const PLANS_V1: &str = "plans.v1";
pub const PROJECTS_V1: &str = "projects.v1";
pub const AGENTS_V1: &str = "agents.v1";
pub const SESSION_V1: &str = "session.v1";
pub const SESSION_FLEET_V1: &str = "session.fleet.v1";
/// Grants live state and its events, session parts, and session host operations whenever requested.
pub const SESSION_STATE_V1: &str = "session.state.v1";
/// The session topic carries the transcript's tail as ordered operations
/// (`SubDelta::Session::transcript`) in place of `live_text` and `live_thinking` events.
pub const SESSION_TRANSCRIPT_V1: &str = "session.transcript.v1";
/// Bounded read-only listing and file previews inside a session's dedicated checkout.
pub const SESSION_FILES_V1: &str = "session.files.v1";
/// Chunked reads of standalone HTML inside a session's dedicated checkout.
pub const SESSION_HTML_V1: &str = "session.html.v1";
pub const SYSTEM_V1: &str = "system.v1";
/// Opt into Runtime rows and category summaries in `system_review`.
pub const SYSTEM_RUNTIME_V1: &str = "system.runtime.v1";
/// Opt into entry point changes in `system_review`. With `system.runtime.v1`,
/// also opt into entries and per-container detector coverage in `system_runtime`.
pub const SYSTEM_ENTRIES_V1: &str = "system.entries.v1";
/// Generic repository diagram summaries, pinned evidence details, and stable-ID Impact rows.
pub const SYSTEM_LENSES_V2: &str = "system.lenses.v2";
/// Data model impact rows in `system_review`.
pub const SYSTEM_DATA_V1: &str = "system.data.v1";
/// Bounded artifact workflow recipes and per-element evidence through `detail.get`.
pub const ARTIFACT_DIAGRAM_V1: &str = "artifact.diagram.v1";
/// With `artifact.diagram.v1`: compare a recipe's evidence with a pinned later commit.
pub const ARTIFACT_DIAGRAM_COMPARE_V1: &str = "artifact.diagram.compare.v1";
/// Resolve task-text citations without their content (`DetailRef::Citation`).
pub const CITATIONS_V1: &str = "citations.v1";
pub const REVIEW_V1: &str = "review.v1";
/// Live gate-attempt projections and bounded log reads through `detail.get`.
pub const GATE_ATTEMPT_V1: &str = "gate-attempt.v1";
pub const TIMELINE_V1: &str = "timeline.v1";
pub const USAGE_V1: &str = "usage.v1";
pub const AUTOMATION_V1: &str = "automation.v1";
pub const SETTINGS_V1: &str = "settings.v1";
pub const ASSISTANT_V1: &str = "assistant.v1";
/// Grants build operations and build rows whenever requested.
pub const BUILD_V1: &str = "build.v1";
pub const CLOSED_V1: &str = "closed.v1";
/// Includes the complete JSON-RPC envelope, not only its result.
pub const APP_CONTROL_MAX: usize = 60 * 1024;
/// Maximum serialized body of a host session part, before the app RPC envelope.
pub const SESSION_PART_MAX: usize = 48 * 1024;
pub const CHANGES_LIMIT_MAX: u16 = 200;

/// Either capability grants access to the shared Assistant topic.
pub fn assistant_topic_enabled(caps: &[String]) -> bool {
    caps.iter()
        .any(|cap| matches!(cap.as_str(), ASSISTANT_V1 | BUILD_V1))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppSessionRequest {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppSession {
    pub device: DeviceInfo,
    pub seen: SeenCursor,
    pub server_time: String,
    /// True on the Unix socket or when the daemon's TCP peer is loopback, false otherwise.
    /// Describes the immediate connection, not client location or authority. Absent on older daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChangesQuery {
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChangesPage {
    #[schemars(with = "Vec<crate::schema_types::Event>")]
    pub events: Vec<Event>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SeenAdvance {
    pub through: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SeenCursor {
    pub through: u64,
    pub at: Option<String>,
}
