//! Chat lifecycle controls and attachments sent with session input.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const ASSISTANT_WARM: &str = "assistant.warm";
pub const ASSISTANT_SESSION: &str = "assistant.session";
pub const QUICK_THREAD: &str = "quick";
pub const MESSAGE_TEXT_MAX: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextRef {
    pub kind: String,
    pub id: String,
    pub rev: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SendAttachment {
    Upload {
        upload: String,
        sha256: String,
    },
    Link {
        url: String,
        title: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AssistantWarm {
    pub thread: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AssistantSessionOp {
    New {
        task: Option<String>,
        profile: Option<String>,
    },
    End {
        session: String,
    },
    SetProfile {
        thread: String,
        profile: String,
    },
    #[serde(other)]
    Other,
}
