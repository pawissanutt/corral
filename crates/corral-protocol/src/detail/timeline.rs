//! Recorded runs and events, without interpolated execution spans.
use corral_types::{Event, Run, Status};
use jiff::Timestamp;
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TimelinePage {
    #[schemars(with = "Vec<crate::schema_types::Run>")]
    pub runs: Vec<Run>,
    #[schemars(with = "Vec<crate::schema_types::Event>")]
    pub events: Vec<Event>,
    /// The tasks the window's runs and events name, their plans, those plans' other
    /// children and the tasks any of them depend on: enough to group lanes by plan, count
    /// a plan's landed children and draw dependency edges. Absent from an older daemon.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<TimelineTask>,
}

/// What the Timeline needs of one task; never its body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TimelineTask {
    pub id: String,
    pub title: String,
    #[schemars(with = "crate::schema_types::Status")]
    pub status: Status,
    /// The plan this task belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// When the task last landed, from the whole event log (not only the window), unless a
    /// later `reopened` undid it. Absent when it never landed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub landed: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Burnup {
    pub scope: String,
    /// `created` and `landed` events are sufficient to draw the burn-up.
    #[schemars(with = "Vec<crate::schema_types::Event>")]
    pub events: Vec<Event>,
}
