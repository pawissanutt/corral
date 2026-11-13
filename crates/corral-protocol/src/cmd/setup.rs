//! Typed setup commands; see the family rules in the parent module.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::FileInput;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum SetupCommand {
    Begin {
        expected_revision: u64,
    },
    Answers {
        file: FileInput,
        expected_revision: u64,
    },
    Propose {
        file: FileInput,
        expected_revision: u64,
    },
    Approve {
        file: FileInput,
        expected_revision: u64,
    },
    Cancel {
        expected_revision: u64,
    },
    SelectionSave {
        file: FileInput,
    },
    #[serde(other)]
    Other,
}

impl SetupCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Begin { .. } => "begin",
            Self::Answers { .. } => "answers",
            Self::Propose { .. } => "propose",
            Self::Approve { .. } => "approve",
            Self::Cancel { .. } => "cancel",
            Self::SelectionSave { .. } => "selection-save",
            Self::Other => "other",
        }
    }
}
