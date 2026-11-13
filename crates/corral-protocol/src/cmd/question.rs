//! Typed question commands; see the family rules in the parent module.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum QuestionCommand {
    Answer {
        key: String,
        rev: u32,
        text: String,
        option: Option<String>,
        basis: Vec<String>,
        scope_note: Option<String>,
        owner: bool,
        json: bool,
    },
    AnswerGroup {
        key: String,
        answers: Option<String>,
        text: String,
        basis: Vec<String>,
        scope_note: Option<String>,
        owner: bool,
        json: bool,
    },
    Escalate {
        key: String,
        rev: u32,
        note: Option<String>,
        kind: Option<String>,
        json: bool,
    },
    Cancel {
        key: String,
        rev: u32,
        reason: String,
        owner: bool,
        json: bool,
    },
    Deadline {
        key: String,
        rev: u32,
        at: String,
        instruction: String,
        json: bool,
    },
    #[serde(other)]
    Other,
}

impl QuestionCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Answer { .. } => "answer",
            Self::AnswerGroup { .. } => "answer-group",
            Self::Escalate { .. } => "escalate",
            Self::Cancel { .. } => "cancel",
            Self::Deadline { .. } => "deadline",
            Self::Other => "other",
        }
    }
}
