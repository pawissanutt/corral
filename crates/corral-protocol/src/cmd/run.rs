//! Typed run commands; see the family rules in the parent module.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::FileInput;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum RunCommand {
    Wait {
        id: String,
        file: FileInput,
    },
    Decision {
        id: String,
        file: FileInput,
    },
    Progress {
        id: String,
        file: FileInput,
        json: bool,
    },
    Checkpoint {
        id: String,
        file: FileInput,
        json: bool,
    },
    CriteriaRegister {
        id: String,
        file: FileInput,
    },
    #[serde(other)]
    Other,
}

impl RunCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Wait { .. } => "wait",
            Self::Decision { .. } => "decision",
            Self::Progress { .. } => "progress",
            Self::Checkpoint { .. } => "checkpoint",
            Self::CriteriaRegister { .. } => "criteria-register",
            Self::Other => "other",
        }
    }
}
