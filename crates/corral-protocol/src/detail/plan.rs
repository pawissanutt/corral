//! A plan has no separate state or revisions: it is a task with children.
use corral_types::Task;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::rows::TaskRow;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlanDetail {
    /// The parent's `body` is the Markdown document.
    #[schemars(with = "crate::schema_types::Task")]
    pub parent: Task,
    /// The children without their `body`, which each child's own task detail serves. Each
    /// child's `depends_on` describes its dependency edges.
    #[schemars(with = "Vec<crate::schema_types::Task>")]
    pub children: Vec<Task>,
    /// Enriched rows for the same children, in the same order on each page, likewise without
    /// their `body`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_rows: Option<Vec<TaskRow>>,
}
