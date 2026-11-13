//! Typed knowledge commands; see the family rules in the parent module.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::FileInput;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum KnowledgeCommand {
    Distill {
        project: Option<String>,
        profile: Option<String>,
        json: bool,
    },
    Apply {
        project: Option<String>,
        file: FileInput,
    },
    Pin {
        project: Option<String>,
        id: String,
        unpin: bool,
    },
    Delete {
        project: Option<String>,
        id: String,
    },
    Put {
        scope: String,
        key: Option<String>,
        id: String,
        file: FileInput,
        expected_hash: Option<String>,
        create: bool,
    },
    Remove {
        scope: String,
        key: Option<String>,
        id: String,
        expected_hash: String,
    },
    #[serde(other)]
    Other,
}

impl KnowledgeCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Distill { .. } => "distill",
            Self::Apply { .. } => "apply",
            Self::Pin { .. } => "pin",
            Self::Delete { .. } => "delete",
            Self::Put { .. } => "put",
            Self::Remove { .. } => "remove",
            Self::Other => "other",
        }
    }
}
