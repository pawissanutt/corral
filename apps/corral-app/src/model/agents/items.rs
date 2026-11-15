//! The transcript's item view-models beside [`super::Tool`]: rendered words, thinking, images,
//! dividers, quiet marker rows, cards and the turn footer, and how much of a continued item the
//! app holds. Every item the daemon's replay serializes maps to one of these; nothing is dropped.

use crate::model::hue::Hue;
use crate::model::review::ParsedHunk;

use super::Action;

/// How much of a recorded item the app holds. An item larger than the replay's preview bound
/// arrives as a preview; its complete content is fetched by its continuation token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum More {
    /// The item is complete as shown.
    Whole,
    /// Only the preview is here. `size` is the complete item's, in words (`12 KB`).
    Preview { size: String },
    /// The complete item is on its way; the preview stays until it arrives.
    Loading { size: String },
    /// The complete item could not be read; the preview stays, with why and a retry.
    Failed {
        size: String,
        reason: String,
        retry: Action,
    },
}

impl More {
    /// The complete item's size, while only its preview shows.
    pub fn size(&self) -> Option<&str> {
        match self {
            More::Whole => None,
            More::Preview { size } | More::Loading { size } | More::Failed { size, .. } => {
                Some(size)
            }
        }
    }
}

/// Words as written: Markdown the renderer lays out (headings, lists, quotes, code, tables).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prose {
    pub markdown: String,
    pub more: More,
    /// Still being written by the live turn: open syntax at its end reads as closed.
    pub streaming: bool,
}

/// The model's reasoning: muted under a `thinking` lead, so it never reads as the answer.
/// Shown by default, as the terminal shows it; the owner can fold it to one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thinking {
    pub text: String,
    pub open: bool,
    /// What the folded row says it hides (`12 lines`).
    pub hidden: String,
    pub more: More,
    pub streaming: bool,
    pub action: Action,
}

/// One block of an opened row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Section {
    /// Recorded text under a label (arguments, output, a record), every line as recorded.
    Text { label: String, text: String },
    /// Changed lines drawn with the review's diff rows.
    Diff {
        label: String,
        hunks: Vec<ParsedHunk>,
        notes: Vec<String>,
    },
    /// Markdown: a summary, a card's text.
    Prose(String),
}

/// An image the session recorded. Desktop and phone draw it; a terminal names it in one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    /// `image/png · 34 KB`.
    pub label: String,
    /// A `data:` URI of the complete bytes, once they are here and match the recorded size.
    pub uri: Option<String>,
    /// Why the image is not drawn although its data is here.
    pub note: Option<String>,
    pub more: More,
    /// Fetches the complete image, while only its preview is here.
    pub action: Option<Action>,
}

/// A rule across the transcript with words in it (a compaction, a branch summary). What came
/// before it stays readable above it; opening it shows the summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divider {
    pub words: String,
    pub note: String,
    pub open: bool,
    pub body: Vec<Section>,
    pub more: More,
    pub action: Action,
}

/// A quiet row the host or the session adds beside the words: a glyph, then muted words
/// (`○ model → opus`, `✗ provider error`). Opening it shows its full text and facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    /// `✓ ✗ ! ○ → ▲`: the shape says the state without its colour.
    pub glyph: &'static str,
    /// Neutral for a quiet row; error only for a failure, warning for a cut or a stall.
    pub hue: Hue,
    pub words: String,
    pub open: bool,
    pub body: Vec<Section>,
    pub more: More,
    /// Opens or folds the row; absent when there is nothing more to show.
    pub action: Option<Action>,
}

/// A titled card a presenter recorded (a Code Mode cell, a job, an owner question snapshot, a
/// custom message): its text, and its complete data when opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub title: String,
    pub text: String,
    pub open: bool,
    /// The card's data, pretty-printed, while open.
    pub body: Vec<Section>,
    pub more: More,
    /// Opens or folds the data; absent when the card has none.
    pub action: Option<Action>,
}

/// An assistant message's foot: model, an unusual stop and usage, then on opening every fact
/// the turn recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Footer {
    pub words: String,
    pub open: bool,
    pub body: Vec<Section>,
    pub more: More,
    pub action: Option<Action>,
}
