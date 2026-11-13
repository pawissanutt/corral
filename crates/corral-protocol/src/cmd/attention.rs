//! Typed attention commands; see the family rules in the parent module.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum AttentionCommand {
    Attend {
        key: String,
        note: String,
        next_check: u64,
        request: Option<String>,
        json: bool,
    },
    Park {
        id: String,
        reason: String,
        resume_when: Vec<String>,
        resume_action: String,
        review_by: u64,
        json: bool,
    },
    Unpark {
        id: String,
        expected_revision: u64,
        json: bool,
    },
    #[serde(other)]
    Other,
}

impl AttentionCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Attend { .. } => "attend",
            Self::Park { .. } => "park",
            Self::Unpark { .. } => "unpark",
            Self::Other => "other",
        }
    }
}
