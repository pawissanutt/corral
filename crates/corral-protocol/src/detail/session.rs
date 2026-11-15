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
    /// The same lines as styled runs, one row per line, when the source carries styling (a live
    /// pane's screen); empty otherwise. A row's run texts concatenate to its line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<Vec<TerminalRun>>,
}

/// Consecutive cells of one terminal row that share a style. An absent colour is the terminal's
/// default for that ground; the client owns the defaults and the indexed palette.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalRun {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<TerminalColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<TerminalColor>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dim: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strike: bool,
    /// Foreground and background swapped (SGR 7), resolved by the client against its defaults.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inverse: bool,
}

/// A terminal colour: an index into the 256-colour palette (0-15 are the client's theme
/// colours), or a 24-bit `[red, green, blue]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TerminalColor {
    Indexed(u8),
    Rgb([u8; 3]),
}
