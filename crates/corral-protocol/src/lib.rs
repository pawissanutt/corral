//! Shared wire contract for the daemon and its clients; independent of store internals.
pub mod board;
pub mod classic;
pub mod cmd;
pub mod frame;
pub mod health;
pub mod read;
pub mod rpc;
#[cfg(test)]
mod schema;
pub mod subscription;

pub use classic::CLASSIC_V1;
pub use health::{DaemonState, Health, LoopFacts, PmState, RuntimeHealth, RuntimeState};
pub use read::{READ, READ_V1};
use serde::{Deserialize, Serialize};

pub const PROTO_MAJOR: u32 = 1;
pub const PROTO_MINOR: u32 = 0;

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
pub const BOARD_V1: &str = "board.v1";

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
