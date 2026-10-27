//! Committed attribution facts, keyed by approved SHA rather than mutable cache files.
use serde::{Deserialize, Serialize};

use super::NewEvent;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LessonApproval {
    pub task: String,
    pub run: Option<String>,
}

pub(crate) fn attribution(event: &NewEvent) -> Option<(&str, LessonApproval)> {
    if event.kind != "approved" || event.detail["strategy"].as_str() == Some("keep_branch") {
        return None;
    }
    Some((
        event.detail["sha"].as_str()?,
        LessonApproval {
            task: event.task.clone()?,
            run: event.detail["run"].as_str().map(str::to_owned),
        },
    ))
}
