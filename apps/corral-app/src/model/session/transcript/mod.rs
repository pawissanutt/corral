//! One transcript model for every session role, child conversation and the Ask panel: the
//! recorded page as the terminal UI replays it (`replay`, tool rows in `tool`, quiet rows, cards
//! and footers in `marks`, call/result pairing in `pairs`), the open turn the host streams
//! (`live`), and this device's own sends that the transcript does not hold yet (`echo`).

use corral_protocol::detail::{DetailBody, SessionPage};

use crate::model::daemon::DetailCache;
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};

use super::Action;
use super::echo::Echo;

mod items;
mod live;
mod marks;
mod pairs;
mod replay;
mod tool;

#[cfg(test)]
mod replay_tests;

pub use items::{Card, Divider, Footer, Marker, More, Picture, Prose, Section, Thinking};
pub use live::{ItemFetch, LiveMessage, LiveSession, LiveTool, LiveToolState, TurnEnd};
pub use replay::{fetches, size, transcript};

/// Terminal line classification, including prompt and success lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineClass {
    Prompt,
    Ok,
    Dim,
    Bold,
    Plain,
}

/// One output or log line, with inline styling where a source mixes styles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: Rich,
    pub class: LineClass,
}

pub(crate) fn line(text: &str, class: LineClass) -> Line {
    Line {
        text: vec![span(text)],
        class,
    }
}

/// Words that say where data stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    Plain(String),
}

/// Whether a surface has content to show, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fill {
    Ready,
    /// Still loading; the words say what ("Loading agents…") once the wait is noticeable.
    Loading(&'static str),
    Empty {
        heading: &'static str,
        paragraph: String,
    },
    /// What failed and what happens now.
    Failed(String),
}

/// A tool call row: the state mark, the tool, what it acted on and how it ended, in one quiet
/// line; opening it shows the arguments, the output, a diff or written content and the
/// complete recorded call and result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub mark: crate::model::board::Mark,
    pub name: String,
    pub summary: String,
    /// `0.4s`, `exit 0`, `failed`, `running`, `not run`.
    pub outcome: String,
    /// Drawn in the failure colour while folded too.
    pub failed: bool,
    /// Muted at the trailing edge: `you`, or which half of the call is not on this page.
    pub note: Option<String>,
    /// Rows under the folded line: a failure's last output lines, a running call's tail.
    pub tail: Vec<Line>,
    /// What the folded row hides, in words the outcome does not already say (`41 lines`).
    pub hidden: Option<String>,
    pub open: bool,
    /// The opened row's blocks.
    pub body: Vec<Section>,
    pub more: More,
    /// Opens or folds the row; absent when there is nothing more to show.
    pub action: Option<Action>,
}

/// One item of a transcript message, in recorded order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageItem {
    /// Styled words a sample writes directly.
    Paragraph(Rich),
    Prose(Prose),
    Thinking(Thinking),
    Tool(Tool),
    Picture(Picture),
    Divider(Divider),
    Marker(Marker),
    Card(Card),
    Question(QuestionCard),
}

/// A question the agent asked, as its transcript recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionCard {
    pub label: String,
    pub question: String,
    pub hint: String,
    /// False once answered: the card is drawn quiet, not in the needs-you colour.
    pub open: bool,
}

/// One transcript message, with avatar and ordered content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The served message id (`<session>/input/<id>`, `<session>/entry/<id>`); empty from an
    /// older daemon.
    pub id: String,
    pub avatar: &'static str,
    pub hue: Hue,
    pub head: Rich,
    pub author: String,
    pub detail: String,
    /// The owner input id the host tagged this message with, shown muted at its first row.
    pub input: Option<String>,
    pub items: Vec<MessageItem>,
    /// An assistant message's model, unusual stop and usage.
    pub footer: Option<Footer>,
    /// The live turn's message, still being written: drawn with the streaming mark.
    pub live: bool,
}

/// Session transcript: one bounded page of messages, this device's sends the page does not
/// hold yet, the controls to page through it, its source's state, then the open turn as the
/// host streams it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub messages: Vec<Message>,
    /// Drawn after the last served message, each until a served message carries its id.
    pub echoes: Vec<Echo>,
    /// Show the messages before these.
    pub older: Option<Action>,
    /// Back to the latest messages, while an earlier page shows.
    pub newer: Option<Action>,
    /// A page is on its way while the shown one stays.
    pub paging: bool,
    pub fill: Fill,
    pub stale: Option<String>,
    pub disclaimer: Option<Note>,
    /// The live turn after the recorded messages, until the recorded transcript holds it.
    pub live: Vec<Message>,
    /// Quiet rows after the live turn: owner input the host holds, how the turn ended.
    pub after: Vec<MessageItem>,
}

/// The served page in a session detail cache.
pub fn page(cache: Option<&DetailCache>) -> Option<&SessionPage> {
    match cache?.body()? {
        DetailBody::Session(page) => Some(page),
        _ => None,
    }
}

/// The page token of the messages before the shown page, when the daemon has older ones.
pub(crate) fn older_token(cache: &DetailCache) -> Option<String> {
    match cache.body()? {
        DetailBody::Session(page) => page.older.clone(),
        _ => None,
    }
}

/// What failed, from the cache's error.
pub(crate) fn failure(cache: Option<&DetailCache>) -> Option<String> {
    cache?
        .error
        .as_ref()
        .map(|error| format!("{} ({})", error.message, error.code))
}
