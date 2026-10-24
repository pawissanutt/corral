//! AgentQuestion v1, stored on the asking run's or review child's `AgentRunState`. Its
//! lifecycle operations belong to question-lifecycle; this module fixes the record shape.
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Starts the run's `blocked_excerpt` while a question holds its task in Needs me; the hold
/// is the question's, so settling it restores Running (worker) or Review (reviewer).
pub const QUESTION_HOLD_PREFIX: &str = "Question ";
pub const QUESTION_TEXT_MAX_BYTES: usize = 4 * 1024;
pub const QUESTION_OPTIONS_MAX: usize = 6;
pub const QUESTION_CONTEXT_REFS_MAX: usize = 16;
pub const QUESTION_TRIAGE_MAX: usize = 16;
pub const ANSWER_TEXT_MAX_BYTES: usize = 8 * 1024;
pub const BLOCKING_QUESTIONS_MAX: usize = 1;
/// Open plus escalated questions per asking run or review child.
pub const PENDING_QUESTIONS_MAX: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    Clarification,
    TechnicalChoice,
    OwnerOnly,
    Permission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionState {
    Open,
    Escalated,
    Answered,
    Expired,
    Withdrawn,
    Cancelled,
}

impl QuestionState {
    pub fn pending(self) -> bool {
        matches!(self, QuestionState::Open | QuestionState::Escalated)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAsked {
    pub request_id: String,
    pub payload_sha256: String,
    pub session_id: String,
    pub generation: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    pub at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionActor {
    Agent,
    Pm,
    Owner,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionTriage {
    pub action: String,
    pub by: QuestionActor,
    pub revision: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerBy {
    Pm,
    Owner,
    OwnerDeadline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    pub by: AnswerBy,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub basis_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_note: Option<String>,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
}

/// An owner-set expiry; void once the question's revision changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionDeadline {
    pub set_by: QuestionActor,
    pub at: Timestamp,
    pub instruction: String,
    pub for_revision: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionWakeRoute {
    PmInbox,
    OmoPush,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxState {
    Pending,
    Posted,
}

/// The PM wake outbox entry, deduplicated by `qwake:<key>:<rev>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionWake {
    pub route: QuestionWakeRoute,
    pub key: String,
    pub state: OutboxState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionTerminal {
    pub state: QuestionState,
    pub reason: String,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentQuestion {
    pub version: u32,
    /// `<task>/<run>[/<review-child>]/Q<n>`, globally unique.
    pub key: String,
    pub revision: u32,
    pub asked: QuestionAsked,
    pub kind: QuestionKind,
    pub blocking: bool,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendation: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_refs: Vec<String>,
    /// A hint only; never an expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needed_by: Option<Timestamp>,
    pub state: QuestionState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triage: Vec<QuestionTriage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<QuestionAnswer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<QuestionDeadline>,
    pub wake: QuestionWake,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unattended_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<QuestionTerminal>,
}

impl AgentQuestion {
    /// Pending in the record. Readers also apply `container_closure`, which makes every question
    /// of a closed container non-pending before terminalization persists it.
    pub fn recorded_pending(&self) -> bool {
        self.terminal.is_none() && self.state.pending()
    }
}
