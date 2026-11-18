//! Owned transcript descriptors populated from the shared feed.

use super::{
    Action, ActionCard, AskUserCard, AttachmentView, Button, Chip, FileChip, FiledCard, SavedCard,
    WaitingBlock,
};
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
    /// How the filing sent from here went.
    pub filed: Option<Chip>,
}

/// An owner-authored PM inbox item and optional folded reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmMessage {
    /// Owner input and its state.
    pub owner: OwnerMessage,
    /// `PMI-<n>`, or "PMI on arrival" while it has none yet.
    pub id: String,
    /// Folded PM outcome in this same thread.
    pub reply: Option<PmReply>,
    /// Non-interruption explanation for queued-for-PM items.
    pub note: Option<&'static str>,
}

/// The PM's outcome of one inbox item, not a separate conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmReply {
    /// Reply time.
    pub at: String,
    /// Outcome text.
    pub text: String,
}

/// Content inside an assistant reply, in its original order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Assistant-written Markdown, laid out as the terminal UI does.
    Markdown(String),
    /// Numbered paragraphs.
    Ordered(Vec<Rich>),
    /// Owner-command class and receipt.
    Action(ActionCard),
    /// An agent's question, answered in place.
    AskUser(AskUserCard),
    /// Assistant-filed Idea with editable generated fields.
    Filed(FiledCard),
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
    /// Answering, stopped, answered-and-filed or needs-you state.
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
    /// PM input plus its optional outcome.
    Pm(PmMessage),
    /// A PM outcome whose inbox item is not in this page of the thread.
    PmReply { id: String, reply: PmReply },
    /// Host/model waiting block.
    Waiting(WaitingBlock),
    /// Locally queued delivery note.
    Queued(QueueNote),
    /// Owner-filed as-written result.
    Saved(SavedCard),
    /// Retained text's withdrawal explanation, or a thread notice.
    Note(String),
}
