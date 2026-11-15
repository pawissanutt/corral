//! Candidate-bound review summaries and annotation threads.
use super::ArtifactRef;
use crate::cmd::annotation::{AnnotationSide, LineRange};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Immutable identity of the coding candidate whose patch is being displayed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewCandidate {
    pub run: String,
    pub base: String,
    pub head: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewDetail {
    pub task: String,
    pub candidate_sha: Option<String>,
    pub base: Option<String>,
    pub state: String,
    pub verdicts: Vec<ReviewVerdict>,
    pub files: Vec<ReviewFile>,
    pub report: Option<String>,
    pub acceptance: Vec<String>,
    pub gate_log_tail: Vec<String>,
    pub artifacts: Vec<ArtifactRef>,
    pub annotations: Vec<ReviewAnnotation>,
    pub approvable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewVerdict {
    pub by: String,
    pub sha: String,
    pub verdict: String,
    pub at: Option<String>,
    pub text: Option<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewFile {
    pub path: String,
    pub added: u32,
    pub deleted: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewAnnotation {
    pub id: String,
    pub revision: u32,
    pub run: String,
    pub base: String,
    pub head: String,
    pub path: String,
    pub side: AnnotationSide,
    pub range: Option<LineRange>,
    pub resolved: bool,
    pub stale: bool,
    pub messages: Vec<AnnotationMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnnotationMessage {
    pub by: String,
    pub at: String,
    pub text: String,
}
