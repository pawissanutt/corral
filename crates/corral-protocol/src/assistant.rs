//! One chat surface, with explicit assistant or PM delivery targets.
use crate::{
    detail::question::{QuestionAnswer, QuestionOption},
    rows::WaitReason,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const ASSISTANT_SEND: &str = "assistant.send";
pub const ASSISTANT_CANCEL: &str = "assistant.cancel";
pub const ASSISTANT_WARM: &str = "assistant.warm";
pub const ASSISTANT_STOP: &str = "assistant.stop";
pub const ASSISTANT_SESSION: &str = "assistant.session";
pub const PROMPT_ANSWER: &str = "prompt.answer";
pub const QUICK_THREAD: &str = "quick";
pub const MESSAGE_TEXT_MAX: usize = 16 * 1024;
pub const LIVE_TEXT_MAX: usize = 32 * 1024;
pub const TURN_TEXT_MAX: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AskTarget {
    Assistant,
    Pm,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextRef {
    pub kind: String,
    pub id: String,
    pub rev: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AssistantSend {
    pub cmd_id: String,
    pub thread: String,
    pub target: AskTarget,
    pub text: String,
    pub typed_at: String,
    pub context: Vec<ContextRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<SendAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SendAck {
    pub entry: String,
    pub state: MessageState,
    pub replayed: bool,
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
pub struct AssistantCancel {
    pub cmd_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CancelAck {
    pub cancelled: bool,
    pub entry: Option<String>,
    pub state: Option<MessageState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AssistantWarm {
    pub thread: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AssistantStop {
    pub session: String,
    pub turn: u64,
}

/// Acceptance is not proof of abort; only canonical `ThreadEntry.completion` attests settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StopAck {
    pub session: String,
    pub turn: u64,
    pub outcome: StopOutcome,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopOutcome {
    Accepted,
    /// The requested turn is no longer running, not necessarily because it was aborted.
    AlreadyStopped,
    Failed,
    #[serde(other)]
    Other,
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
    TakeOver {
        session: String,
    },
    /// Fence the session: its host stays up and queued messages wait. An owner message or
    /// `resume` lifts it.
    Pause {
        session: String,
    },
    Resume {
        session: String,
    },
    SetProfile {
        thread: String,
        profile: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PromptAnswer {
    pub cmd_id: String,
    pub key: String,
    pub rev: u32,
    pub choice: PromptChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromptChoice {
    Once,
    Hour,
    Session,
    Deny,
    Confirm,
    Decline,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MessageState {
    Sent,
    Waiting {
        reason: WaitReason,
    },
    Delivered,
    Late,
    QueuedForPm,
    PmDelivered,
    PmReplied,
    Saved {
        task: String,
    },
    Withdrawn,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EntryPart {
    /// Semantic message ID; answers use the live stream's `<message>/answer/0` anchor.
    pub id: String,
    /// Zero-based transport part position, independent of page boundaries.
    pub index: u32,
    pub total: u32,
    /// Whole canonical content length in UTF-8 bytes at this revision.
    pub bytes: u64,
    /// No more content will be appended. All `total` parts must still be collected.
    pub complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnCompletion {
    Completed,
    Aborted,
    Failed,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadEntry {
    pub id: String,
    pub rev: u64,
    pub at: String,
    /// Original owner-typed RFC 3339 time; `at` remains the canonical receipt/order time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed_at: Option<String>,
    pub kind: EntryKind,
    pub device: Option<String>,
    pub via: Option<String>,
    pub text: Option<String>,
    pub state: Option<MessageState>,
    pub model: Option<String>,
    /// One semantic answer's amount, carried only by part zero. None means unknown, never zero.
    pub cost_usd: Option<f64>,
    pub tools: Vec<ToolLine>,
    /// Absent on older servers; continuation IDs remain readable as legacy transport parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<EntryPart>,
    /// Canonical host settlement, never inferred from a Stop request or a generic done stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<TurnCompletion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_basis: Option<crate::rows::CostBasis>,
    pub links: Vec<ContextRef>,
    pub card: Option<EntryCard>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<crate::attachment::AttachmentFact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Message,
    Answer,
    Idea,
    PmMessage,
    PmReply,
    Action,
    Question,
    Prompt,
    Notice,
    Roll,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolLine {
    pub tool: String,
    pub input: String,
    pub output: Option<String>,
    pub elapsed_ms: Option<u64>,
    pub failed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryCard {
    Idea {
        task: String,
        title: String,
        labels: Vec<String>,
        criteria: Vec<String>,
        duplicates: Vec<ContextRef>,
        words: String,
    },
    Action {
        /// Historical display data, not a command that can be submitted.
        command: serde_json::Value,
        class: String,
        outcome: Option<serde_json::Value>,
        input: Option<String>,
    },
    Question {
        key: String,
        rev: u32,
        options: Vec<QuestionOption>,
        answer: Option<QuestionAnswer>,
        /// Whether the historical question card offered an owner reply.
        #[serde(default)]
        accepts_text: bool,
    },
    Prompt {
        key: String,
        rev: u32,
        tool: Option<String>,
        path: Option<String>,
        preview: String,
        choices: Vec<PromptChoice>,
        needs_presence: bool,
    },
    PmItem {
        id: String,
        /// Canonical PM inbox revision used by pm.inbox-withdraw, not ThreadEntry.rev.
        #[serde(default)]
        revision: u32,
    },
    #[serde(other)]
    Other,
}
