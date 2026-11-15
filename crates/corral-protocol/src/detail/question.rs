//! Canonical questions, consequences, attribution and resolvable evidence.
use crate::rows::{InboxKind, InboxState};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionDetail {
    pub key: String,
    pub rev: u32,
    pub state: InboxState,
    pub kind: InboxKind,
    pub blocking: bool,
    pub task: Option<String>,
    pub plan: Option<String>,
    pub from: QuestionFrom,
    pub asked_at: String,
    pub needed_by: Option<String>,
    pub text: String,
    pub options: Vec<QuestionOption>,
    pub recommendation: Option<String>,
    pub place: QuestionPlace,
    /// At most five blocks, each bounded to 8 KiB by the reader.
    pub evidence: Vec<EvidenceBlock>,
    pub thread: Vec<Followup>,
    pub history: Vec<QuestionHistory>,
    pub answer: Option<QuestionAnswer>,
    pub deadline: Option<QuestionDeadline>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionFrom {
    pub role: String,
    pub session: Option<String>,
    pub profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionOption {
    pub label: String,
    pub next: Option<String>,
    pub unblocks: Vec<String>,
    pub cost: Option<String>,
    pub recommended: bool,
    /// Zero-based index into this detail's evidence blocks.
    pub evidence: Option<usize>,
    /// No option-specific consequence was recorded or safely derived.
    #[serde(default)]
    pub consequence_missing: bool,
    /// At least one consequence field was inferred from canonical task dependencies.
    #[serde(default)]
    pub consequence_derived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionPlace {
    pub goal: Option<String>,
    pub position: Option<PlanPosition>,
    pub waiting: Vec<QuestionWaiting>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanPosition {
    pub plan: String,
    pub layer: u32,
    pub layers: u32,
    pub landed: u32,
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionWaiting {
    pub id: String,
    pub title: String,
    pub kind: WaitKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaitKind {
    You,
    Plan,
    Info,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceBlock {
    pub source_ref: String,
    pub title: String,
    pub body: EvidenceBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceBody {
    Diff {
        path: String,
        lines: Vec<String>,
        truncated: bool,
    },
    Excerpt {
        path: Option<String>,
        start_line: Option<u32>,
        lines: Vec<String>,
        truncated: bool,
    },
    LogTail {
        lines: Vec<String>,
        truncated: bool,
    },
    /// Catalog reference only; bytes use the bounded Artifact detail reader.
    Image {
        artifact: super::ArtifactRef,
    },
    Unresolved {
        reason: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Followup {
    pub by: String,
    pub via: String,
    pub at: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionHistory {
    pub by: String,
    pub via: Option<String>,
    pub at: String,
    pub text: String,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionAnswer {
    pub by: String,
    pub via: String,
    pub device: Option<String>,
    pub at: String,
    pub text: String,
    pub option: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionDeadline {
    pub at: String,
    pub instruction: String,
}
