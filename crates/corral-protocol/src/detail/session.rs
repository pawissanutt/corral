//! Bounded transcript pages and read-only terminal captures.
use super::question::{QuestionAnswer, QuestionOption};
use crate::assistant::ToolLine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionPage {
    pub id: String,
    pub role: String,
    pub task: Option<String>,
    pub run: Option<String>,
    pub machine: Option<String>,
    pub messages: Vec<SessionMessage>,
    pub older: Option<String>,
    pub replyable: bool,
    pub reply_refusal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMessage {
    pub author: String,
    pub at: Option<String>,
    pub items: Vec<SessionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionItem {
    Text {
        text: String,
    },
    Tool {
        tool: ToolLine,
    },
    QuestionCard {
        key: String,
        rev: u32,
        text: String,
        options: Vec<QuestionOption>,
        answer: Option<QuestionAnswer>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalSnapshot {
    /// At most 200 captured lines, not a writable or live terminal.
    pub lines: Vec<String>,
    pub captured_at: String,
    pub live: bool,
}
