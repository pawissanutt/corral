//! Typed setup commands; see the family rules in the parent module.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum SetupCommand {
    #[serde(other)]
    Other,
}

impl SetupCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Other => "other",
        }
    }
}
