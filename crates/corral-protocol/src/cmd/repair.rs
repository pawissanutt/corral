//! Typed repair commands; see the family rules in the parent module.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::FileInput;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum RepairCommand {
    Triage {
        id: String,
        file: FileInput,
        owner: bool,
    },
    Finding {
        id: String,
        file: FileInput,
    },
    Diagnose {
        id: String,
        owner: bool,
    },
    Diagnosed {
        id: String,
        file: FileInput,
    },
    Renew {
        id: String,
        file: FileInput,
    },
    #[serde(other)]
    Other,
}

impl RepairCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Triage { .. } => "triage",
            Self::Finding { .. } => "finding",
            Self::Diagnose { .. } => "diagnose",
            Self::Diagnosed { .. } => "diagnosed",
            Self::Renew { .. } => "renew",
            Self::Other => "other",
        }
    }
}
