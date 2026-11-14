//! Annotation inputs mirror canonical candidate and thread revision checks.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationSide {
    New,
    Old,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum AnnotationCommand {
    Open {
        id: String,
        key: String,
        expected_run: String,
        base: String,
        head: String,
        path: String,
        side: AnnotationSide,
        range: Option<LineRange>,
        text: String,
    },
    Reply {
        id: String,
        key: String,
        thread: String,
        expected_revision: u32,
        text: String,
    },
    Resolve {
        id: String,
        key: String,
        thread: String,
        expected_revision: u32,
        text: String,
    },
    Reopen {
        id: String,
        key: String,
        thread: String,
        expected_revision: u32,
        text: String,
    },
    #[serde(other)]
    Other,
}

impl AnnotationCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Open { .. } => "open",
            Self::Reply { .. } => "reply",
            Self::Resolve { .. } => "resolve",
            Self::Reopen { .. } => "reopen",
            Self::Other => "other",
        }
    }
}
