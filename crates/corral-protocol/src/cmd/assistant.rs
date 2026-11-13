//! Typed assistant commands; see the family rules in the parent module.
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::FileInput;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum AssistantCommand {
    Create {
        profile: String,
        focus: Option<String>,
        pane: Option<String>,
        herdr_socket: Option<PathBuf>,
        json: bool,
    },
    Summary {
        id: String,
        file: FileInput,
        json: bool,
    },
    PermissionsRevoke {
        id: String,
        grant: Option<String>,
        all: bool,
        json: bool,
    },
    #[serde(other)]
    Other,
}

impl AssistantCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::Summary { .. } => "summary",
            Self::PermissionsRevoke { .. } => "permissions-revoke",
            Self::Other => "other",
        }
    }
}
