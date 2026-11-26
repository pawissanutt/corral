//! Task detail over the engine's stored facts.
use corral_types::{Artifact, Check, Event, PendingReviewVerdict, Run, Task};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskDetail {
    #[schemars(with = "crate::schema_types::Task")]
    pub task: Task,
    #[schemars(with = "Vec<crate::schema_types::Run>")]
    pub runs: Vec<Run>,
    #[schemars(with = "Vec<crate::schema_types::Check>")]
    pub checks: Vec<Check>,
    #[schemars(with = "Vec<crate::schema_types::Artifact>")]
    pub artifacts: Vec<Artifact>,
    #[schemars(with = "Vec<crate::schema_types::Event>")]
    pub events: Vec<Event>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<crate::schema_types::PendingReviewVerdict>")]
    pub pending_review_verdict: Option<PendingReviewVerdict>,
}
