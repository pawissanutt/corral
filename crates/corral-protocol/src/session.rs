//! Direct owner controls addressed to a public session scope, never to a pane.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SESSION_CANCEL: &str = "session.cancel";
pub const SESSION_PAUSE: &str = "session.pause";
pub const SESSION_RESUME: &str = "session.resume";
pub const SESSION_TERMINATE: &str = "session.terminate";
pub const SESSION_RELAUNCH: &str = "session.relaunch";
pub const SESSION_REWIND: &str = "session.rewind";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionTarget {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCancel {
    pub id: String,
    /// If present, cancel only this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCancelReply {
    pub cancelled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionPauseReply {
    pub paused: bool,
    #[serde(flatten)]
    pub turn: SessionCancelReply,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionResumeReply {
    pub paused: bool,
}

/// Input cancellation names the submitted message; direct cancellation
/// names the session and optionally the turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AssistantCancelRequest {
    Session(SessionCancel),
    Input(crate::assistant::AssistantCancel),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionRelaunched {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionRewind {
    pub id: String,
    #[serde(flatten)]
    pub action: RewindAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RewindAction {
    List,
    Turn { turn: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RewindReply {
    List {
        checkpoints: Vec<RewindCheckpoint>,
        rewinds: Vec<Rewound>,
    },
    Turn(Rewound),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RewindCheckpoint {
    pub id: String,
    pub phase: String,
    pub turn: u64,
    pub commit: Option<String>,
    pub head: Option<String>,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub leaf: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rewound {
    pub turn: u64,
    pub checkpoint: String,
    pub rescue_ref: String,
    pub from_head: String,
    pub head: String,
    pub at: String,
}

/// `prompt.answer` accepts a Pi question ID and a choice, free text, or both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionPromptAnswer {
    pub session: String,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionPromptAnswered {
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PromptAnswerRequest {
    Session(SessionPromptAnswer),
    Legacy(crate::assistant::PromptAnswer),
}
