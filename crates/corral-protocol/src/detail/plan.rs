//! A plan has no separate state or revisions: it is a task with children.
use corral_types::Task;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlanDetail {
    /// The parent's `body` is the Markdown document.
    #[schemars(with = "crate::schema_types::Task")]
    pub parent: Task,
    /// Each child's `depends_on` describes its dependency edges.
    #[schemars(with = "Vec<crate::schema_types::Task>")]
    pub children: Vec<Task>,
}
