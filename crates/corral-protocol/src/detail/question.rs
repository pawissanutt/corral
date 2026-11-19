//! Question cards embedded in chat and session transcripts.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
pub struct QuestionAnswer {
    pub by: String,
    pub via: String,
    pub device: Option<String>,
    pub at: String,
    pub text: String,
    pub option: Option<String>,
}
