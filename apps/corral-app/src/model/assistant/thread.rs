//! Owned transcript descriptors populated from the shared feed.

use super::{Action, AttachmentView, Button, Chip, FileChip, WaitingBlock};
use crate::model::text::Rich;

/// An owner message, with origin and delivery state independently available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerMessage {
    /// Name shown in the message header.
    pub author: &'static str,
    /// Header provenance including the `app:` prefix when appropriate.
    pub provenance: String,
    /// Delivery/display time.
    pub at: String,
    /// Owner's exact text.
    pub text: String,
    /// Delivery, waiting or terminal state, as its outlined tag.
    pub tag: Chip,
    /// Additional attribution such as the offline typing time.
    pub extra: Option<String>,
    /// Shared attachment or bounded context carried by the input.
    pub attachment: Option<String>,
    /// The images, files and links the message carries, as served (or as this device sent).
    pub files: Vec<FileChip>,
    /// The attachment whose canonical readback is open under the message.
    pub opened: Option<AttachmentView>,
    /// File as idea: files the message's words as an Idea task, with no model.
    pub file: Option<Button>,
    /// How the filing sent from here went: the new task's id, or the refusal.
    pub filed: Option<Chip>,
}

/// Content inside an assistant reply, in its original order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Assistant-written Markdown, laid out as the terminal UI does.
    Markdown(String),
    /// Numbered paragraphs.
    Ordered(Vec<Rich>),
    /// Host startup explanation.
    Starting { state: Chip, text: String },
}

/// One expandable tool-activity row: tool, command and elapsed time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRow {
    /// Tool name.
    pub kind: String,
    /// Command sent to the tool.
    pub command: String,
    /// Duration of this call, empty when unreported.
    pub elapsed: String,
    /// The call failed.
    pub failed: bool,
}

/// A record cited under the completed reply, and what pressing it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLink {
    /// Link text.
    pub label: String,
    /// Opens the record (or shows the fixture search outcome).
    pub action: Action,
}

/// An assistant message with the identity of the model that actually wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantMessage {
    /// Stable identity of the reply in its thread, for per-reply view state.
    pub key: String,
    /// Session id in the author header.
    pub id: String,
    /// Reply time.
    pub at: String,
    /// Historical model name, unaffected by selecting the next message's model.
    pub model: String,
    /// Answering, stopped or needs-you state.
    pub state: Option<Chip>,
    /// Ordered reply body, including any cards.
    pub body: Vec<Body>,
    /// Expandable tool summary, absent when the turn used no tools.
    pub tool_summary: Option<String>,
    /// Expandable tool rows.
    pub tools: Vec<ToolRow>,
    /// Whether to display the streaming caret after the visible paragraphs.
    pub streaming: bool,
    /// Optional Stop button.
    pub stop: Option<Button>,
    /// Cost and token count for this ask.
    pub cost: Option<String>,
    /// Completed-turn supporting records.
    pub sources: Vec<SourceLink>,
    /// Partial-answer retention explanation after Stop.
    pub stopped_note: Option<&'static str>,
    /// Snapshot/wording disclosure.
    pub illustrative: Option<&'static str>,
}

impl AssistantMessage {
    /// Heading for supporting record links, shown only after a completed turn.
    pub fn sources_label(&self) -> Option<&'static str> {
        (!self.sources.is_empty()).then_some("Rests on")
    }
}

/// A roll divider that preserves access to the previous read-only transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollDivider {
    /// Summary of the previous generation and counts.
    pub text: String,
    /// Opens the earlier transcript, when it is known.
    pub transcript: Option<Button>,
}

/// A queue note and its non-expiring-message escape hatches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueNote {
    /// Delivery, late-authority and context-change explanation.
    pub text: String,
    /// File and/or withdraw actions.
    pub buttons: Vec<Button>,
}

/// Top-level transcript item; render in sequence, without deriving state from prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadItem {
    /// Generation boundary and summary.
    Roll(RollDivider),
    /// Owner input.
    Owner(OwnerMessage),
    /// Assistant output and cards.
    Assistant(AssistantMessage),
    /// Host/model waiting block.
    Waiting(WaitingBlock),
    /// Locally queued delivery note.
    Queued(QueueNote),
    /// Retained text's withdrawal explanation, or a thread notice.
    Note(String),
}
