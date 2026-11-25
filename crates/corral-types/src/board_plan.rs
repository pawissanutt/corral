//! The board-plan request and compact, durable filing receipt.

use serde::{Deserialize, Serialize};

use crate::{Status, TaskDifficulty};

pub const MAX_PLAN_BYTES: usize = 128 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardPlan {
    pub title: String,
    pub project: String,
    pub why: String,
    pub tasks: Vec<BoardPlanTask>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardPlanTask {
    pub key: String,
    pub title: String,
    pub project: Option<String>,
    pub kind: Option<String>,
    pub profile: Option<String>,
    pub size: Option<String>,
    pub difficulty: Option<TaskDifficulty>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub depends: Vec<String>,
    pub body: String,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct BoardPlanReceipt {
    pub parents: Vec<BoardPlanParent>,
    pub tasks: Vec<BoardPlanFiledTask>,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct BoardPlanParent {
    pub project: String,
    pub id: String,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct BoardPlanFiledTask {
    pub key: String,
    pub project: String,
    pub id: String,
    pub status: Status,
    pub depends_on: Vec<String>,
}
