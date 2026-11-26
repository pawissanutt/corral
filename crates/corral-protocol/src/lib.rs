//! Shared wire contract for the daemon and its clients; independent of store internals.
pub mod app;
pub mod assistant;
pub mod attachment;
#[cfg(test)]
mod attachment_tests;
pub mod build;
pub mod config;
pub mod detail;
pub mod device;
pub mod diagram;
pub mod frame;
pub mod health;
pub mod host;
pub mod op;
pub mod pane;
pub mod rows;
pub mod rpc;
#[cfg(test)]
mod schema;
mod schema_types;
pub mod session;
pub mod session_host;
pub mod session_state;
pub mod subscription;

pub use app::{
    AGENTS_V1, ASSISTANT_V1, AUTOMATION_V1, BUILD_V1, CHANGES_V1, CLOSED_V1, PLANS_V1, PROJECTS_V1,
    REVIEW_V1, SESSION_FLEET_V1, SESSION_STATE_V1, SESSION_TRANSCRIPT_V1, SETTINGS_V1, TIMELINE_V1,
    USAGE_V1,
};
pub use corral_types::{
    Applied, Artifact, Blocked, Check, Event, Next, Op, Refusal, Run, SessionStatus, Status, Task,
    Usage, UsageBy, UsageGroup, UsageSummary, UsageTotal,
};
pub use health::{DaemonState, Health};
use serde::{Deserialize, Serialize};

pub const PROTO_MAJOR: u32 = 1;
pub const PROTO_MINOR: u32 = 1;

pub const HELLO: &str = "hello";
pub const PING: &str = "ping";
pub const BYE: &str = "bye";
pub const NUDGE: &str = "nudge";
pub const HEALTH_GET: &str = "health.get";
pub const DAEMON_STOP: &str = "daemon.stop";
pub const SUB_OPEN: &str = "sub.open";
pub const SUB_DELTA: &str = "sub.delta";
pub const SUB_RESET: &str = "sub.reset";
pub const SUB_ACK: &str = "sub.ack";
pub const SUB_CLOSE: &str = "sub.close";
pub const TASKS_V1: &str = "tasks.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Proto {
    pub major: u32,
    pub minor: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ClientInfo {
    pub kind: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Hello {
    pub proto: Proto,
    pub caps: Vec<String>,
    pub client: ClientInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DaemonInfo {
    pub version: String,
    pub store: String,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Welcome {
    pub proto: Proto,
    pub caps: Vec<String>,
    pub daemon: DaemonInfo,
    pub health: Health,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Ping {}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Pong {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Bye {
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Nudge {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NudgeAck {
    pub pass_seq: u64,
    /// True only after a full pass that started after receipt of this nudge completed
    /// within the requested wait. Finishing an already-running pass does not qualify.
    pub completed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HealthGet {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Stop {
    pub drain: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Stopped {}
