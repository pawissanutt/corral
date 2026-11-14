//! Task owner revisions, current execution and plan placement.
use super::{ArtifactRef, question::PlanPosition};
use crate::board::RailStep;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskDetail {
    pub id: String,
    /// The canonical owner revision, never a projection row revision.
    pub owner_revision: String,
    pub title: String,
    pub status: String,
    pub body_excerpt: String,
    pub stages: [RailStep; 4],
    pub dependencies: Vec<TaskDependency>,
    pub progress: Option<TaskProgress>,
    pub owner_next: Option<String>,
    pub hold: Option<TaskHold>,
    pub park: Option<TaskHold>,
    pub artifacts: Vec<ArtifactRef>,
    pub run: Option<TaskRun>,
    pub plan_context: Option<PlanPosition>,
    pub log_tail: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskDependency {
    pub id: String,
    pub title: String,
    pub status: String,
    pub satisfied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskProgress {
    pub completed: u32,
    pub total: u32,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskHold {
    pub reason: Option<String>,
    pub since: Option<String>,
    pub by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskRun {
    pub id: String,
    pub session: Option<String>,
    pub profile: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub base: Option<String>,
    pub head: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}
