//! Candidate review owner commands; execution stays in canonical ops.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum ReviewCommand {
    Approve {
        id: String,
        sha: String,
    },
    RequestChanges {
        id: String,
        text: String,
        escalate: bool,
    },
    Verdict {
        id: String,
        sha: String,
        verdict: String,
        notes: Option<String>,
    },
    Discard {
        id: String,
    },
    #[serde(other)]
    Other,
}

impl ReviewCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Approve { .. } => "approve",
            Self::RequestChanges { .. } => "request-changes",
            Self::Verdict { .. } => "verdict",
            Self::Discard { .. } => "discard",
            Self::Other => "other",
        }
    }
}
