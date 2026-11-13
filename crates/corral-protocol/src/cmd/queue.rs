//! Typed queue commands; see the family rules in the parent module.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum QueueCommand {
    Add {
        id: String,
        profile: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        base: Option<String>,
        headless: bool,
    },
    Remove {
        id: String,
    },
    Pause {},
    Resume {},
    Admit {
        project: String,
        off: bool,
    },
    #[serde(other)]
    Other,
}

impl QueueCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Add { .. } => "add",
            Self::Remove { .. } => "remove",
            Self::Pause {} => "pause",
            Self::Resume {} => "resume",
            Self::Admit { .. } => "admit",
            Self::Other => "other",
        }
    }
}
