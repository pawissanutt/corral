//! Hash-guarded canonical configuration edits.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum ConfigCommand {
    Set {
        key: String,
        value: String,
        expected_hash: String,
    },
    /// Clone only a server-configured profile; never accepts host commands or raw TOML.
    ProfileAdd {
        name: String,
        from: String,
        model: Option<String>,
        effort: Option<String>,
        expected_hash: String,
    },
    ProfileRemove {
        name: String,
        expected_hash: String,
    },
    #[serde(other)]
    Other,
}

impl ConfigCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Set { .. } => "set",
            Self::ProfileAdd { .. } => "profile-add",
            Self::ProfileRemove { .. } => "profile-remove",
            Self::Other => "other",
        }
    }
}
