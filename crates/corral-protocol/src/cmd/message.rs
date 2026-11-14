//! Assistant-target message actions. PM withdrawal uses pm.inbox-withdraw.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum MessageCommand {
    FileAsIdea {
        entry: String,
    },
    Withdraw {
        entry: String,
    },
    #[serde(other)]
    Other,
}

impl MessageCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::FileAsIdea { .. } => "file-as-idea",
            Self::Withdraw { .. } => "withdraw",
            Self::Other => "other",
        }
    }
}
