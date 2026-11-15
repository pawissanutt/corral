//! Content-free Web Push, bound to the authenticated device, off by default.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const PUSH_V1: &str = "push.v1";
pub const PUSH_STATUS: &str = "push.status";
pub const PUSH_SUBSCRIBE: &str = "push.subscribe";
pub const PUSH_UNSUBSCRIBE: &str = "push.unsubscribe";

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PushKind {
    NeedsYou,
    PlanAwaitingAcceptance,
    Stuck,
    Escalated,
    DaemonUnhealthy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushSubscription {
    /// HTTPS push-service URL, never the daemon endpoint.
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushSubscribe {
    pub subscription: PushSubscription,
    pub kinds: Vec<PushKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushStatus {
    pub available: bool,
    pub vapid_public: Option<String>,
    pub enabled: bool,
    pub kinds: Vec<PushKind>,
    pub last_delivered: Option<String>,
    pub last_error: Option<String>,
}
