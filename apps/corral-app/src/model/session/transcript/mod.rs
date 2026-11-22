//! One transcript model for every session role, child conversation and the Ask panel: the
//! recorded page as the terminal UI replays it (`replay`), tool rows per tool family (`tool` and
//! its presenters `tool_files`, `tool_jobs`, `tool_corral`, `tool_local`), runs of settled calls
//! folded into one row (`group`), Code Mode cells (`code_cell`), build cards (`build_cards`),
//! owner and agent bubbles and host-made quiet rows (`bubbles`), quiet rows, cards and footers
//! (`marks`), call/result pairing (`pairs`), the open turn the host streams (`live`), and this
//! device's own sends that the transcript does not hold yet (`echo`).

use corral_protocol::detail::{DetailBody, SessionPage};

use crate::model::Id;
use crate::model::daemon::DetailCache;
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};

use super::Action;
use super::echo::Echo;

mod bubbles;
mod build_cards;
mod build_entries;
mod code_cell;
mod group;
mod items;
mod journal;
mod live;
mod marks;
mod overlay;
mod pairs;
mod replay;
mod tool;
mod tool_corral;
mod tool_files;
mod tool_jobs;
mod tool_local;

#[cfg(test)]
mod bubbles_tests;
#[cfg(test)]
mod build_entries_tests;
#[cfg(test)]
mod code_cell_tests;
#[cfg(test)]
mod group_tests;
#[cfg(test)]
mod replay_tests;
#[cfg(test)]
mod tool_tests;

pub use build_cards::{
    BuildCard, ExitLine, ExitStep, FinishCard, GateRow, PlanCard, ReviewRow, SliceBack, SliceRow,
    TaskCard,
};
pub use build_entries::{ExitEntry, FiledEntry, Told};
pub use code_cell::{CellOutput, CellRow, CodeCell};
pub use group::Group;
pub use items::{
    Answered, AnsweredQuestion, Card, Divider, Footer, Marker, More, Picture, Prose, Section,
    Thinking,
};
pub use live::{ItemFetch, LiveMessage, LiveSession, LiveTool, LiveToolState, TurnEnd};
pub use replay::{fetches, size, transcript};
pub use tool::{ToolState, ToolView};

/// The row key that opens every row and group of the transcript at once (Ctrl+O, the ⋯ menu's
/// `Expand all tool rows`): `Action::Expand { key: EXPAND_ALL, .. }` toggles it, and while it is
/// set a press on one row folds that row alone.
pub const EXPAND_ALL: &str = "*all";

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

/// A tool call row in the terminal UI's grammar:
/// `<state> <verb> <target> · <outcome> · <detail>   <label> <hidden> <time> <view>`. A failed
/// call keeps its reason rows under it, a running one the tail of its output; opening it shows
/// the presenter's full view (a diff, the written file, the whole output, a corral read's
/// fields), then the arguments and the complete recorded call and result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub state: ToolState,
    /// The verb: the tool in words (`read`, `wait`, `Attached`).
    pub name: String,
    /// What it acted on (`src/lib.rs:1-40`, `j1-4 · cargo test`).
    pub summary: String,
    /// How it ended (`27 lines`, `exit 0`, `refused`); empty while there is nothing to say.
    pub outcome: String,
    /// The outcome's colour: neutral (muted), `Fail` for a failure, `Held` for a warning.
    pub outcome_hue: Hue,
    /// A command's last output line, a first search result: shown when there is room.
    pub detail: Option<String>,
    /// Muted at the trailing edge: `you`, `timeout 2m`, a job id, which half of the call is not
    /// on this page.
    pub note: Option<String>,
    /// What the folded row hides, in words the outcome does not already say (`41 lines`).
    pub hidden: Option<String>,
    /// The call's own time (`0.4s`, `1m 12s`); absent below 100 ms and while unknown.
    pub time: Option<String>,
    /// The view the row opens (`Agents`, `Changes`).
    pub view: Option<ToolView>,
    /// Rows under the folded line: a failure's reason, a running call's tail, a write's newest
    /// lines.
    pub tail: Vec<Line>,
    pub open: bool,
    /// The opened row's blocks.
    pub body: Vec<Section>,
    pub more: More,
    /// Opens or folds the row; absent when there is nothing more to show.
    pub action: Option<Action>,
    /// What the row folds by, when it stands in a run of calls of its kind.
    pub fold: group::Fold,
}

/// One item of a transcript message, in recorded order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageItem {
    /// Styled words a sample writes directly.
    Paragraph(Rich),
    Prose(Prose),
    Thinking(Thinking),
    Tool(Tool),
    /// A run of settled calls of one kind, folded into one row.
    Group(Group),
    /// A Code Mode cell and the calls it recorded.
    Cell(CodeCell),
    /// A build tool's own card (`task`, `build_finish`, `board_plan`).
    Build(BuildCard),
    /// An `ask_user` answer: the questions and what the owner chose.
    Answered(Answered),
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

/// How a message is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// The owner's words (or a brief, a steer, an agent's message in a child conversation) on
    /// the raised bubble, with its lead in place of an avatar.
    Bubble,
    /// An agent's words under its avatar and author.
    Agent,
    /// What the host or the session added (run marks, host notices, model changes): quiet rows
    /// with no avatar and no header.
    Quiet,
}

/// One transcript message, with its look and ordered content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The served message id (`<session>/input/<id>`, `<session>/entry/<id>`); empty from an
    /// older daemon.
    pub id: String,
    /// What the reading position holds on to: the served id, else the recorded time and author;
    /// stable across re-reads, new rows and widths.
    pub anchor: String,
    pub kind: MessageKind,
    pub avatar: &'static str,
    pub hue: Hue,
    /// The header: the author (or a bubble's lead, `you`, `you → a2`, `brief from parent`,
    /// `a2 → parent`), then its time.
    pub head: Rich,
    pub author: String,
    pub detail: String,
    /// Muted at the right of the first row: where an owner message came from (`this device`,
    /// `from <device name>`, `another device`, `typed in the terminal`), or the owner input id
    /// an assistant forwarded. Never this device's own command id.
    pub tag: Option<String>,
    /// What the tag stands for, opened from it: a device the roster does not name, by its id.
    pub tag_facts: Option<TagFacts>,
    pub items: Vec<MessageItem>,
    /// Each item's stable key, from the recorded item it starts with (counted from its record's
    /// end, so a split record's earlier items leave it alone): the reader holds on to the item
    /// they read across rows arriving above and below it. Empty when items have no such source.
    pub keys: Vec<String>,
    /// An assistant message's model, unusual stop and usage.
    pub footer: Option<Footer>,
    /// The live turn's message, still being written: drawn with the streaming mark.
    pub live: bool,
}

/// The facts behind a message's tag, shown under its first row once opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagFacts {
    pub open: bool,
    pub body: Vec<Section>,
    pub action: Action,
}

/// Session transcript: newest recorded messages plus any earlier pages already read, this
/// device's unmatched sends, its source's state and the open turn as the host streams it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub messages: Vec<Message>,
    /// Source message anchors in display order, with the number of rows through each one.
    /// Several can share a bubble (and even a folded tool row).
    message_ends: Vec<(String, usize)>,
    /// The first recorded row: it changes whenever rows arrive above the shown ones, also when
    /// they are a record's earlier items read from the page before (a record split across
    /// pages keeps its message and anchor).
    pub first: Option<String>,
    /// Drawn after the last served message, each until a served message carries its id.
    pub echoes: Vec<Echo>,
    /// Show the messages before these.
    pub older: Option<Action>,
    /// Identifies the next older page even when the preceding page displayed no new rows.
    pub older_cursor: Option<String>,
    /// Back to the latest messages, when a source shows a separate earlier page.
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
    /// The session scope this transcript is of (a child's conversation has its own).
    pub scope: Id,
    /// Where the reader was when they last left this transcript.
    pub reading: Reading,
}

/// Where a reader is in a session's transcript, kept per scope across leaving and reopening
/// it: following its end (the default for a transcript never read), or scrolled up to a row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Reading {
    #[default]
    End,
    At {
        /// The anchor of the row at the top of the view.
        key: String,
        /// How far into that row, in points.
        offset: i32,
        /// The transcript's row count when the reader left the end: what arrived since is
        /// counted from it.
        rows: usize,
    },
}

/// Where a view of the transcript starts when it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    End,
    /// At the row the reader left, while the transcript still shows it.
    Row {
        key: String,
        offset: i32,
        rows: usize,
    },
}

/// What a view following the transcript reads of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Following {
    pub scope: Id,
    pub rows: usize,
    pub resume: Resume,
    /// The last source message's anchor and how many rows follow it.
    pub tail: Option<(String, usize)>,
}

impl Transcript {
    /// What a view needs to follow this transcript and to open where its reader left it. A row
    /// the shown page no longer holds (an earlier page, a rewind) opens at the end.
    pub fn following(&self) -> Following {
        let resume = match &self.reading {
            Reading::At { key, offset, rows }
                if self
                    .messages
                    .iter()
                    .chain(&self.live)
                    .any(|message| message.anchor == *key) =>
            {
                Resume::Row {
                    key: key.clone(),
                    offset: *offset,
                    rows: *rows,
                }
            }
            _ => Resume::End,
        };
        let rows = self.rows();
        Following {
            scope: self.scope.clone(),
            rows,
            tail: self
                .message_ends
                .last()
                .map(|(anchor, through)| (anchor.clone(), rows - through)),
            resume,
        }
    }

    fn trailing_rows(&self) -> usize {
        self.echoes.len() + self.after.len() + self.live.iter().map(message_rows).sum::<usize>()
    }

    /// Rows from messages after `anchor`, including when they share its displayed bubble.
    /// Used on a prepend so replies arriving in the same update still count as new.
    pub fn rows_below(&self, anchor: &str) -> Option<usize> {
        let (_, through) = self
            .message_ends
            .iter()
            .rev()
            .find(|(key, _)| key == anchor)?;
        Some(self.rows() - through)
    }

    /// How many rows the transcript shows: the jump control counts what arrives while the
    /// reader is scrolled up by this.
    pub fn rows(&self) -> usize {
        self.messages.iter().map(message_rows).sum::<usize>() + self.trailing_rows()
    }
}

fn message_rows(message: &Message) -> usize {
    message.items.len() + usize::from(message.footer.is_some())
}

/// The served page in a session detail cache.
pub fn page(cache: Option<&DetailCache>) -> Option<&SessionPage> {
    match cache?.body()? {
        DetailBody::Session(page) => Some(page),
        _ => None,
    }
}

/// The token of the page before the loaded transcript, when the daemon has older messages.
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
