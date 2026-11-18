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
pub const REVIEW_V1: &str = "review.v1";
pub const TIMELINE_V1: &str = "timeline.v1";
pub const USAGE_V1: &str = "usage.v1";
pub const SETTINGS_V1: &str = "settings.v1";
pub const ASSISTANT_V1: &str = "assistant.v1";
pub const CLOSED_V1: &str = "closed.v1";
/// Includes the complete JSON-RPC envelope, not only its result.
pub const APP_CONTROL_MAX: usize = 60 * 1024;
pub const CHANGES_LIMIT_MAX: u16 = 200;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppSessionRequest {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppSession {
    pub device: DeviceInfo,
    pub seen: SeenCursor,
    pub server_time: String,
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
