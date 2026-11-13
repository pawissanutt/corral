//! Typed inbox commands; see the family rules in the parent module.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum InboxCommand {
    IndexOwner {
        key: String,
        limit: u8,
        json: bool,
    },
    Recover {
        key: String,
        resend: bool,
        abandon: bool,
        submit: bool,
        discard: bool,
    },
    Mark {
        id: String,
        observed: bool,
        cancel: bool,
        reason: String,
    },
    Flush {
        key: String,
    },
    Retry {
        id: String,
    },
    #[serde(other)]
    Other,
}

impl InboxCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::IndexOwner { .. } => "index-owner",
            Self::Recover { .. } => "recover",
            Self::Mark { .. } => "mark",
            Self::Flush { .. } => "flush",
            Self::Retry { .. } => "retry",
            Self::Other => "other",
        }
    }
}
