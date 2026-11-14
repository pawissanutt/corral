//! Recorded execution spans; unavailable phases are explicit, never estimated.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimelineBy {
    Task,
    Agent,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TimelinePage {
    pub lanes: Vec<TimelineLane>,
    pub owner_actions: Vec<TimelineAction>,
    pub pm: Vec<TimelineAction>,
    pub last_visit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TimelineLane {
    pub id: String,
    pub title: String,
    pub segments: Vec<TimelineSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TimelineSegment {
    pub kind: TimelineKind,
    pub from: Option<String>,
    pub to: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimelineKind {
    Run,
    Gate,
    Review,
    Land,
    Ask,
    Held,
    Idle,
    Unavailable,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TimelineAction {
    pub at: String,
    pub task: Option<String>,
    pub text: String,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Burnup {
    pub scope: String,
    pub points: Vec<BurnupPoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BurnupPoint {
    pub at: String,
    pub total: u32,
    pub landed: u32,
}
