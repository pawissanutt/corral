//! Typed plan commands; see the family rules in the parent module.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::FileInput;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum PlanCommand {
    Cancel {
        id: String,
        owner_ok: bool,
        reason: Option<String>,
        json: bool,
    },
    AddMember {
        tracker: String,
        member: String,
        json: bool,
    },
    MoveMember {
        tracker: String,
        member: String,
        to: String,
        gate_file: Option<FileInput>,
        reason: Option<String>,
        json: bool,
    },
    Policy {
        id: String,
        policy: String,
    },
    #[serde(other)]
    Other,
}

impl PlanCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Cancel { .. } => "cancel",
            Self::AddMember { .. } => "add-member",
            Self::MoveMember { .. } => "move-member",
            Self::Policy { .. } => "policy",
            Self::Other => "other",
        }
    }
}
