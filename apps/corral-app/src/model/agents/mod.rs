//! Agents wall and session state, reducers and presentation data.
//!
//! The assistant module supplies the quick thread before the agent tiles, and the sessions and
//! Filed from chat blocks after them, on both desktop and phone. The presentation types here are
//! owned so the same screens draw the fixture preview (`fixture`) and the daemon's agent rows and
//! session pages (`live`, `live_session`).

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::cmd::{Command, task::TaskCommand};
use corral_protocol::detail::{DetailBody, DetailGet, DetailRef};

use crate::model::board::{Mark, Rail};
use crate::model::daemon;
use crate::model::effect::{Effect, Env, Page};
use crate::model::hue::Hue;
use crate::model::nav::{Destination, Form};
use crate::model::text::{Rich, span};
use crate::model::{Act, ActStatus, Id, Live, Selection, act};

mod data;
mod fixture;
mod live;
mod live_session;
pub mod screen;

pub use data::AGENTS;
pub use fixture::{session, wall};
pub use live::wall as live_wall;
pub use live_session::session as live_session;

/// The agent's current activity, used for the wall chip and phone dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Live,
    Working,
    Asking,
    Gating,
    Reviewing,
    Idle,
    Waiting,
    Ended,
    Unavailable,
    /// A state this build does not know.
    Unknown,
}

impl AgentStatus {
    /// Chip text and state colour.
    pub fn chip(self) -> (&'static str, Hue) {
        match self {
            Self::Live => ("live", Hue::Corral),
            Self::Working => ("working", Hue::Worker),
            Self::Asking => ("needs you", Hue::You),
            Self::Gating => ("gating", Hue::Corral),
            Self::Reviewing => ("reviewing", Hue::Worker),
            Self::Idle => ("idle", Hue::Neutral),
            Self::Waiting => ("waiting", Hue::Held),
            Self::Ended => ("ended", Hue::Neutral),
            Self::Unavailable => ("unavailable", Hue::Fail),
            Self::Unknown => ("unknown", Hue::Neutral),
        }
    }
}

/// How fresh a tail or terminal is: current, or an older capture that must never pass for a
/// stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    Live,
    Snapshot,
}

impl Feed {
    /// Short phone label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Snapshot => "snapshot",
        }
    }
    /// Wall tile label.
    pub fn wall_label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Snapshot => "snapshot · 20s ago",
        }
    }
}

/// One fixture agent on the workstation, including its complete illustrative tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Agent {
    pub id: &'static str,
    pub role: &'static str,
    pub task: Option<&'static str>,
    pub title: &'static str,
    pub profile: &'static str,
    pub runtime: &'static str,
    pub pane: &'static str,
    pub state: AgentStatus,
    pub doing: &'static str,
    pub since: &'static str,
    pub machine: &'static str,
    pub tail: &'static [&'static str],
    pub feed: Feed,
}

/// The selected session surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Transcript,
    Terminal,
}
/// The wall/list setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Wall,
    List,
}
/// Session and wall selection, retained across navigation. The fixture session id is the
/// preview's; daemon sessions are selected by the root `Selection`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub session: &'static str,
    /// The fixture preview's Transcript | Terminal choice; it opens on the transcript, as the
    /// mockup does.
    pub view: View,
    /// The owner's Transcript | Terminal choice for daemon sessions; it opens on the pane's
    /// live screen. Kept apart from `view` because the root's switch key toggles without
    /// knowing the source, and each source has its own first view.
    pub live_view: View,
    pub mode: Mode,
    /// Expanded tool calls of daemon transcripts, by `Tool::key`.
    pub expanded: BTreeSet<String>,
    /// The latest reply command per daemon session, kept to show its outcome; one key per
    /// intended reply, never replaced after an uncertain result.
    pub replies: BTreeMap<Id, Id>,
    /// Sessions whose transcript shows an earlier page rather than the latest messages. Such a
    /// page is not replaced by the latest until the owner asks for it.
    pub earlier: BTreeSet<Id>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            session: "T-868",
            view: View::Transcript,
            live_view: View::Terminal,
            mode: Mode::Wall,
            expanded: BTreeSet::new(),
            replies: BTreeMap::new(),
            earlier: BTreeSet::new(),
        }
    }
}
impl State {
    /// Select a fixture session without resetting its transcript/terminal choice.
    pub fn open(&mut self, id: &'static str) {
        self.session = id;
    }
    /// Toggle transcript and terminal.
    pub fn toggle_view(&mut self) {
        self.view = flip(self.view);
    }
    /// Toggle a daemon session's transcript and terminal.
    pub fn toggle_live_view(&mut self) {
        self.live_view = flip(self.live_view);
    }
    /// Handle an action in the fixture preview.
    pub fn act(&mut self, action: Action, env: Env) -> Vec<Effect> {
        match action {
            Action::Open(id) => {
                self.open(id);
                vec![Effect::Open(Page::Session(id))]
            }
            Action::Question => vec![Effect::Open(Page::Question)],
            Action::Tool => vec![Effect::Toast("Tool call expanded".into())],
            Action::Send(_) => {
                let id = fixture::effective_id(self.session);
                let text = if env.form == Form::Phone {
                    "Sent · via=app:pawis-iphone".to_string()
                } else {
                    format!("Sent to {id} via corral send · actor=owner via=app:pawis-desktop")
                };
                vec![Effect::Toast(text)]
            }
            other => self.shared(other),
        }
    }

    /// Handle an action on daemon data. Every command goes through the root's typed `Act`
    /// path; nothing is simulated locally.
    pub fn act_live(
        &mut self,
        action: Action,
        feed: &daemon::Feed,
        selection: &Selection,
        _env: Env,
    ) -> Vec<Effect> {
        match action {
            Action::OpenSession(id) => vec![Effect::OpenLive(Live::Session(id))],
            Action::OpenQuestion(key) => vec![Effect::OpenLive(Live::Question(key))],
            Action::OpenThread(id) => vec![Effect::OpenLive(Live::Thread(id))],
            Action::Expand(key) => {
                if !self.expanded.remove(&key) {
                    self.expanded.insert(key);
                }
                vec![]
            }
            Action::Older(id) => match older(feed, &id) {
                Some(effect) => {
                    self.earlier.insert(id);
                    vec![effect]
                }
                None => vec![],
            },
            Action::Newer(id) => {
                self.earlier.remove(&id);
                vec![Effect::Detail(DetailGet {
                    target: DetailRef::Session { id: id.to_string() },
                    etag: None,
                    page: None,
                })]
            }
            Action::Refresh(target) => {
                if let DetailRef::Session { id } = &target {
                    self.earlier.remove(id.as_str());
                }
                vec![Effect::Detail(DetailGet {
                    etag: feed
                        .detail(&target)
                        .and_then(|cache| cache.pages.first())
                        .map(|page| page.etag.clone()),
                    target,
                    page: None,
                })]
            }
            Action::Recover(key) => vec![Effect::Recover(key)],
            Action::Send(text) => match selection.session.clone() {
                Some(session) => self.reply(session, text, feed),
                None => vec![],
            },
            Action::SetView(view) => {
                self.live_view = view;
                vec![]
            }
            Action::ToggleView => {
                self.toggle_live_view();
                vec![]
            }
            Action::Open(_) | Action::Question | Action::Tool => vec![],
            other => self.shared(other),
        }
    }

    /// Navigation and the wall setting both sources share, and the fixture's view switches
    /// (a daemon session's are handled in `act_live`).
    fn shared(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::Back => vec![Effect::Go(Destination::Agents)],
            Action::SetView(view) => {
                self.view = view;
                vec![]
            }
            Action::SetMode(mode) => {
                self.mode = mode;
                vec![]
            }
            Action::ToggleView => {
                self.toggle_view();
                vec![]
            }
            _ => vec![],
        }
    }

    /// Sends the typed words to the session's task as owner input (`task.reply`). Only a page the
    /// daemon marked replyable, with a task to reply to, accepts it; a reply being prepared or in
    /// flight blocks another.
    fn reply(&mut self, session: Id, text: String, feed: &daemon::Feed) -> Vec<Effect> {
        let text = text.trim();
        if text.is_empty() || reply_busy(self, &session, feed) {
            return vec![];
        }
        let target = DetailRef::Session {
            id: session.to_string(),
        };
        let Some(task) = feed
            .detail(&target)
            .and_then(daemon::DetailCache::body)
            .and_then(|body| match body {
                DetailBody::Session(page) if page.replyable => page.task.clone(),
                _ => None,
            })
        else {
            return vec![];
        };
        let act = Act::new(
            format!("Reply to {task}"),
            Command::Task(TaskCommand::Reply {
                id: task.clone(),
                text: text.to_owned(),
            }),
        )
        .about(Live::Session(session.clone()));
        self.replies.insert(session, act.key.clone());
        vec![Effect::Act(act)]
    }
}

fn flip(view: View) -> View {
    match view {
        View::Transcript => View::Terminal,
        View::Terminal => View::Transcript,
    }
}

/// True while the session's latest reply is being prepared, queued or in flight; its key must
/// not be replaced. An unknown outcome does not block: the reply box says it may have arrived,
/// and sending again is the owner's explicit new message.
pub fn reply_busy(state: &State, session: &str, feed: &daemon::Feed) -> bool {
    state
        .replies
        .get(session)
        .and_then(|key| act::status(feed, key))
        .is_some_and(|status| {
            matches!(
                status,
                ActStatus::Previewing
                    | ActStatus::Confirming
                    | ActStatus::Queued
                    | ActStatus::Pending
                    | ActStatus::Running
            )
        })
}

/// The page of messages before the shown ones, when the daemon offered one and none is loading.
/// It replaces the shown page: transcripts are read a bounded page at a time.
fn older(feed: &daemon::Feed, session: &Id) -> Option<Effect> {
    let target = DetailRef::Session {
        id: session.to_string(),
    };
    let cache = feed.detail(&target)?;
    if cache.loading {
        return None;
    }
    let token = live_session::older_token(cache)?;
    Some(Effect::Detail(DetailGet {
        target,
        etag: None,
        page: Some(token),
    }))
}

/// Actions attached to tiles, controls, tool calls and the reply box. The first group belongs to
/// the fixture preview; the second carries daemon ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Fixture: open a fixture session.
    Open(&'static str),
    /// Fixture: open the fixture handover.
    Question,
    Back,
    SetView(View),
    SetMode(Mode),
    /// Fixture: a terminal key on the phone (the preview sends nothing).
    Key(&'static str),
    ToggleView,
    /// Fixture: a tool call row.
    Tool,
    /// Send the reply field's words.
    Send(String),
    OpenSession(Id),
    OpenQuestion(Id),
    /// The assistant thread a pane-less assistant session is answered in.
    OpenThread(Id),
    /// Expand or fold a daemon tool call's output.
    Expand(String),
    /// Show the page of messages before the shown ones.
    Older(Id),
    /// Return to the latest messages.
    Newer(Id),
    /// Revalidate a session page or terminal capture.
    Refresh(DetailRef),
    /// Ask the daemon for the outcome of an uncertain reply, under the same key.
    Recover(Id),
}

/// Terminal line classification, including prompt and success lines in the wall tails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineClass {
    Prompt,
    Ok,
    Dim,
    Bold,
    Plain,
}
/// One terminal line, with inline styling where the mockup mixes styles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: Rich,
    pub class: LineClass,
}
fn line(text: &str, class: LineClass) -> Line {
    Line {
        text: vec![span(text)],
        class,
    }
}
/// A tail or output line, classed by what it starts with or reports.
fn tail_line(text: &str) -> Line {
    let class = if text.contains("PASS") || text.contains("ok ") || text.contains("pass") {
        LineClass::Ok
    } else if ["$", "Edit", "Bash", "Read", "wake", "read", "Ask"]
        .iter()
        .any(|p| text.starts_with(p))
    {
        LineClass::Prompt
    } else {
        LineClass::Plain
    };
    line(text, class)
}

/// Words that say where data stands: the fixture's illustrative tag, or a plain note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    Illustrative(&'static str),
    Plain(String),
}

/// Whether a surface has content to show, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fill {
    Ready,
    Loading,
    Empty {
        heading: &'static str,
        paragraph: String,
    },
    /// What failed and what happens now.
    Failed(String),
}

/// One wall tile or phone row, with its actionable session and last four tail lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    /// The session id, for the `agent-<id>` test id.
    pub id: String,
    /// The id shown: the task, or the session for agents without one.
    pub label: String,
    pub role: String,
    pub title: String,
    pub doing: String,
    pub profile: String,
    pub runtime: String,
    pub pane: String,
    /// Reported or estimated cost and tokens, when known.
    pub usage: Option<String>,
    pub feed: Feed,
    /// The tail's tag on a tile.
    pub feed_label: String,
    /// The tail's tag in rows.
    pub feed_short: String,
    /// "since 21:40".
    pub since: String,
    /// "21:40".
    pub since_short: String,
    pub chip: &'static str,
    pub hue: Hue,
    pub doing_hue: Hue,
    pub tail: Vec<Line>,
    pub action: Action,
}
/// Agents page content; quick thread and assistant blocks are provided by the assistant module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wall {
    pub title: &'static str,
    pub subtitle: &'static str,
    pub machine: String,
    pub machine_detail: String,
    pub machine_note: Option<&'static str>,
    pub other_machines: &'static str,
    pub phone_machine_detail: String,
    pub phone_machine_note: Option<&'static str>,
    pub count: usize,
    pub tiles: Vec<Tile>,
    pub fill: Fill,
    /// The last known rows are shown while their source is unreachable.
    pub stale: Option<String>,
    pub footer: Note,
    pub mode: Mode,
    pub mode_labels: [&'static str; 2],
}

/// A tool call row. Daemon output shows when the row is expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub kind: String,
    pub argument: String,
    pub result: String,
    pub mark: Mark,
    pub output: Vec<Line>,
    /// Whether pressing the row shows or hides output; spoken with the row.
    pub expanded: Option<bool>,
    pub action: Action,
}
/// A paragraph, tool call or inline handover within a transcript message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageItem {
    Paragraph(Rich),
    Tool(Tool),
    Question(QuestionCard),
}
/// Inline owner-only handover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionCard {
    pub label: String,
    pub question: String,
    pub button: &'static str,
    pub hint: String,
    /// False once answered: the card is drawn quiet, not in the needs-you colour.
    pub open: bool,
    pub action: Action,
}
/// One transcript message, with avatar and ordered content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub avatar: &'static str,
    pub hue: Hue,
    pub head: Rich,
    pub author: String,
    pub detail: String,
    pub items: Vec<MessageItem>,
}
/// Session transcript: one bounded page of messages, the controls to page through it, and its
/// source's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub messages: Vec<Message>,
    /// Show the messages before these.
    pub older: Option<Action>,
    /// Back to the latest messages, while an earlier page shows.
    pub newer: Option<Action>,
    /// A page is on its way while the shown one stays.
    pub paging: bool,
    pub fill: Fill,
    pub stale: Option<String>,
    pub disclaimer: Option<Note>,
}

/// Whether the reply box takes words, and what it says when it does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyMode {
    Open,
    /// A reply is in flight; Send waits for its outcome.
    Busy,
    /// Replies do not go to this session; the reason, and where to answer instead.
    Closed {
        reason: String,
        action: Option<(&'static str, Action)>,
    },
}

/// Reply box copy, state and the outcome of the latest reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub placeholder: String,
    pub accessibility_label: &'static str,
    pub button: &'static str,
    pub hint: Rich,
    pub question_hint: String,
    pub mode: ReplyMode,
    /// The latest reply's outcome in its tone.
    pub status: Option<(Hue, String)>,
    /// "Check delivery" for a reply whose outcome is unknown.
    pub recover: Option<Action>,
    /// The key of the latest delivered reply: the field clears once when it changes.
    pub delivered: Option<String>,
}
/// One labeled fact in the desktop run sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFact {
    pub label: &'static str,
    pub value: FactValue,
}
/// A fact is either styled text or the task's rail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactValue {
    Text(Rich),
    Rail(Rail),
}
/// Desktop run sidebar, including the task's rail and ordered steps.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub title: &'static str,
    pub facts: Vec<RunFact>,
    pub steps_title: &'static str,
    pub steps: Vec<(Mark, String)>,
}
/// Terminal toolbar and complete lines. Snapshot data must never be presented as a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminal {
    pub feed: Feed,
    pub heading: String,
    pub status: String,
    pub input: String,
    pub lines: Vec<Line>,
    pub cursor: bool,
    /// The lines are the pane's screen rows as read: drawn as received, one row each, never
    /// wrapped or recoloured, so frames and status bars line up as in the pane.
    pub verbatim: bool,
    /// The same screen rows with the pane's colours and styles, when the screen carried them;
    /// empty otherwise, and then `lines` are drawn.
    pub styled: Vec<screen::Row>,
    /// What the empty surface says instead of lines (no capture yet, none recorded).
    pub empty: Option<String>,
    /// Fixture: the phone's terminal keys. Daemon captures take no keys.
    pub phone_keys: Vec<&'static str>,
    /// Revalidate the capture.
    pub refresh: Option<Action>,
}
/// Session header and selected surface; both surfaces are supplied for switching. A session
/// without a pane has no terminal, and no switch.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub sub: String,
    pub switch_key: &'static str,
    pub switch_hint: &'static str,
    pub phone_title: String,
    pub back_label: &'static str,
    pub desktop_back_label: &'static str,
    pub back: Action,
    pub view: View,
    pub switch_labels: [&'static str; 2],
    pub switch_actions: [Action; 2],
    pub toggle: Action,
    /// Why the page shows what it does: the session left the wall, or its source is stale.
    pub notice: Option<(Hue, String)>,
    pub transcript: Transcript,
    pub terminal: Option<Terminal>,
    pub reply: Reply,
    pub run_details: Run,
}

#[cfg(test)]
mod tests {
    use super::{State, View};
    #[test]
    fn toggling_twice_does_not_leave_terminal_selected() {
        let mut state = State::default();
        state.toggle_view();
        state.toggle_view();
        assert_eq!(state.view, View::Transcript);
    }
    #[test]
    fn opening_another_session_does_not_reset_view() {
        let mut state = State::default();
        state.toggle_view();
        state.open("T-854");
        assert_eq!(state.view, View::Terminal);
    }
}
