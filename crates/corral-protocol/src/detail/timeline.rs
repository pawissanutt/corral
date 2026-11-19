//! Recorded runs and events, without interpolated execution spans.
use corral_types::{Event, Run};
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Burnup {
    pub scope: String,
    /// `created` and `landed` events are sufficient to draw the burn-up.
    #[schemars(with = "Vec<crate::schema_types::Event>")]
    pub events: Vec<Event>,
}
