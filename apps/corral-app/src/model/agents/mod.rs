//! Agents roster and session state, reducers and presentation data.
//!
//! The roster lists corrald's live runs (`agents.v1`: role, profile, model, session status and
//! quiet time) beside the assistant sessions, the quick thread pinned first. Each run's row,
//! panel and page carry its Session menu (`menu`): Reply, Stop, Retry, Focus pane, then its
//! task's Actions menu. The page shows the run's terminal (its pane's screen, or a headless
//! session's log tail) with the pane's keys bar and type field (`pane`). Recorded transcripts
//! are read only from a corrald that serves them (`session.v1`). The presentation types here are
//! owned so every data source uses the same feed-backed roster and session pages.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::detail::{DetailGet, DetailRef};

use crate::model::board::{Mark, Rail};
use crate::model::daemon;
use crate::model::effect::{Effect, Env};
use crate::model::hue::Hue;
use crate::model::nav::Destination;
use crate::model::text::{Rich, span};
use crate::model::{ActStatus, Id, Live, Selection, act};

mod control;
mod fleet;
mod items;
mod live;
mod live_reply;
mod live_session;
mod menu;
mod pane;
mod prompt;
mod replay;
mod replay_marks;
mod replay_pairs;
#[cfg(test)]
mod replay_tests;
mod replay_tool;
mod roster;
mod roster_panel;
#[cfg(test)]
mod roster_tests;
pub mod screen;

pub use control::{
    Control, ControlAsk, Kind, RewindPoint, RewindSheet, ask, control_verbs, controls, kind,
    rewind_sheet, status as control_status,
};
pub use fleet::{Fleet, FleetEntry, FleetLine, FleetSection};
pub use items::{Card, Divider, Footer, Marker, More, Picture, Prose, Section, Thinking};
pub(crate) use live::placement;
pub use live_session::session as live_session;
pub use menu::{SESSION_MENU_LABEL, session_verbs};
pub use pane::{KEYS, PaneAsk, PaneInput, PaneKey, PaneSent, focus, focus_refusal};
pub use prompt::{Prompt, QUESTION_CARD, answer as answer_prompt, prompt};
pub use replay::fetches;
pub(crate) use replay::size;
pub use roster::{RoleTab, Roster, RosterRole, RosterRow, roster};
pub use roster_panel::{
    Recent, RecentEntry, RecentState, RosterPanel, RosterPanelTab, RosterPanelTabItem, RosterWait,
    roster_panel,
};

/// A run's session as the runner last observed it, used for the roster chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Working,
    /// Stopped at a prompt or a question on its screen.
    Blocked,
    Idle,
    /// The runner no longer finds the session.
    Gone,
}

impl AgentStatus {
    /// Chip text and state colour.
    pub fn chip(self) -> (&'static str, Hue) {
        match self {
            Self::Working => ("working", Hue::Worker),
            Self::Blocked => ("blocked", Hue::You),
            Self::Idle => ("idle", Hue::Neutral),
            Self::Gone => ("gone", Hue::Fail),
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
}

/// The selected session surface. A run's page opens on its terminal: corrald serves its pane,
/// while recorded transcripts arrive with the chat host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    Transcript,
    #[default]
    Terminal,
}
/// Session and roster interaction state; root selection owns the session identity.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    /// The transcript or terminal view the owner chose; until then a pi session opens on its
    /// transcript and any other run on its terminal.
    pub live_view: Option<View>,
    /// Roster selection belongs to this screen, not the full-session route.
    pub roster_session: Option<Id>,
    pub roster_role: RosterRole,
    pub roster_tab: RosterPanelTab,
    /// Transcript rows the owner opened, by stable item key (a tool's call id). A thinking
    /// block shows by default, so its key here folds it instead.
    pub expanded: BTreeSet<String>,
    /// The latest reply operation per run session, kept to show its outcome; one key per
    /// intended reply.
    pub replies: BTreeMap<Id, Id>,
    /// Sessions whose transcript shows an earlier page rather than the latest messages. Such a
    /// page is not replaced by the latest until the owner asks for it.
    pub earlier: BTreeSet<Id>,
    /// The latest pane request per run session (keys or Focus pane), whose outcome it shows.
    pub pane: BTreeMap<Id, PaneSent>,
    /// The latest session control per session, whose outcome it shows.
    pub controls: BTreeMap<Id, control::ControlSent>,
    /// The open rewind sheet.
    pub rewind: Option<control::Rewinding>,
}
impl State {
    /// Shows the other view of `session` than the one it shows now.
    pub fn toggle_view(&mut self, feed: &daemon::Feed, session: &str) {
        self.live_view = Some(flip(self.view_for(feed, session)));
    }
    /// The view a session page opens on: the owner's choice, else the transcript for a pi
    /// session and the terminal for any other.
    pub fn view_for(&self, feed: &daemon::Feed, session: &str) -> View {
        self.live_view.unwrap_or_else(|| {
            if feed.agents.get(session).is_some_and(live::is_pi) {
                View::Transcript
            } else {
                View::Terminal
            }
        })
    }
    /// Handle an action on daemon data. Every operation goes through the root's typed `Act`
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
            Action::OpenThread(id) => vec![Effect::OpenLive(Live::Thread(id))],
            Action::SelectRoster(id) => {
                self.roster_session = Some(id);
                self.roster_tab = RosterPanelTab::Summary;
                vec![]
            }
            Action::ClearRoster => {
                self.roster_session = None;
                vec![]
            }
            Action::SetRosterRole(role) => {
                self.roster_role = role;
                vec![]
            }
            Action::SetRosterTab(tab) => {
                self.roster_tab = tab;
                vec![]
            }
            Action::OpenRosterTarget(target) => vec![Effect::OpenLive(target)],
            Action::Expand { key, fetch } => {
                let opening = !self.expanded.remove(&key);
                if opening {
                    self.expanded.insert(key);
                }
                match fetch {
                    Some((session, token)) if opening && wanted(feed, &token) => {
                        vec![Effect::SessionItem { session, token }]
                    }
                    _ => vec![],
                }
            }
            Action::Fetch { session, token } => {
                if wanted(feed, &token) {
                    vec![Effect::SessionItem { session, token }]
                } else {
                    vec![]
                }
            }
            Action::Watch { session, on } => vec![Effect::Watch { session, on }],
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
                vec![reread(feed, target)]
            }
            Action::Send(text) => match selection.session.clone() {
                Some(session) => self.reply(session, text, feed),
                None => vec![],
            },
            Action::SetView(view) => {
                self.live_view = Some(view);
                vec![]
            }
            Action::Pane {
                session,
                text,
                keys,
            } => self.type_into(session, text, keys, feed),
            Action::ToggleView => {
                let shown = selection
                    .session
                    .as_ref()
                    .map_or(View::Terminal, |session| self.view_for(feed, session));
                self.live_view = Some(flip(shown));
                vec![]
            }
            Action::Control { session, control } => vec![Effect::Control {
                session,
                key: self.ids.next_id(),
                control,
                confirmed: false,
            }],
            Action::RewindTo(turn) => self.rewind_to(turn),
            Action::CloseRewind => {
                self.rewind = None;
                vec![]
            }
            Action::Answer {
                session,
                question,
                option,
                text,
            } => prompt::answer(&session, question, option, text)
                .into_iter()
                .collect(),
            Action::Back => vec![Effect::Go(Destination::Agents)],
        }
    }
}

fn flip(view: View) -> View {
    match view {
        View::Transcript => View::Terminal,
        View::Terminal => View::Transcript,
    }
}

/// Whether a continuation should be fetched: it was never asked for, or its last read failed.
/// A fetch in flight or done is not asked for again.
fn wanted(feed: &daemon::Feed, token: &str) -> bool {
    feed.items
        .get(token)
        .is_none_or(|fetch| matches!(fetch, crate::model::session_feed::ItemFetch::Failed(_)))
}

/// True while the session's latest reply is in flight; its key must not be replaced. A settled
/// one does not block: sending again is the owner's explicit new message.
pub fn reply_busy(state: &State, session: &str, feed: &daemon::Feed) -> bool {
    state
        .replies
        .get(session)
        .and_then(|key| act::status(feed, key))
        .is_some_and(|status| matches!(status, ActStatus::Pending | ActStatus::Running))
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

/// Revalidate a session page or terminal capture with the etag of the page shown.
fn reread(feed: &daemon::Feed, target: DetailRef) -> Effect {
    Effect::Detail(DetailGet {
        etag: feed
            .detail(&target)
            .and_then(|cache| cache.pages.first())
            .map(|page| page.etag.clone()),
        target,
        page: None,
    })
}

/// Actions attached to roster rows, the panel, tool calls and the reply box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    SelectRoster(Id),
    ClearRoster,
    SetRosterRole(RosterRole),
    SetRosterTab(RosterPanelTab),
    OpenRosterTarget(Live),
    Back,
    SetView(View),
    ToggleView,
    /// Send the reply field's words to the run's task (`reply`).
    Send(String),
    OpenSession(Id),
    /// The assistant thread a roster assistant row is answered in.
    OpenThread(Id),
    /// Open or fold a transcript row by its stable key. Opening a row whose complete content
    /// is not here yet also fetches it, by the session and continuation token.
    Expand {
        key: String,
        fetch: Option<(Id, String)>,
    },
    /// Fetch the complete item behind a transcript preview (again, after a failure).
    Fetch {
        session: Id,
        token: String,
    },
    /// Acquire or release one watcher of a session's live turn, while its page is open.
    Watch {
        session: Id,
        on: bool,
    },
    /// Show the page of messages before the shown ones.
    Older(Id),
    /// Return to the latest messages.
    Newer(Id),
    /// Revalidate a session page or terminal capture.
    Refresh(DetailRef),
    /// Type `text` literally into run session `session`'s pane, then press `keys` (herdr's
    /// key names).
    Pane {
        session: Id,
        text: String,
        keys: Vec<String>,
    },
    /// Send a session control (the confirm sheet first for one that asks).
    Control {
        session: Id,
        control: Control,
    },
    /// Rewind the open rewind sheet's session to the start of this turn, after one confirm.
    RewindTo(u64),
    CloseRewind,
    /// Answer the agent's question with an option, words, or both.
    Answer {
        session: Id,
        question: String,
        option: Option<String>,
        text: Option<String>,
    },
}

/// Terminal line classification, including prompt and success lines.
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
    pub mark: Mark,
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
    /// Styled words the fixture preview writes directly.
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
    pub avatar: &'static str,
    pub hue: Hue,
    pub head: Rich,
    pub author: String,
    pub detail: String,
    pub items: Vec<MessageItem>,
    /// An assistant message's model, unusual stop and usage.
    pub footer: Option<Footer>,
    /// The live turn's message, still being written: drawn with the streaming mark.
    pub live: bool,
}
/// Session transcript: one bounded page of messages, the controls to page through it, its
/// source's state, then the open turn as the host streams it.
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
    /// The live turn after the recorded messages, until the recorded transcript holds it.
    pub live: Vec<Message>,
    /// Quiet rows after the live turn: owner input the host holds, how the turn ended.
    pub after: Vec<MessageItem>,
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
    pub mode: ReplyMode,
    /// The latest reply's outcome in its tone.
    pub status: Option<(Hue, String)>,
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
    /// Revalidate the capture.
    pub refresh: Option<Action>,
}
/// The strip under a session's head: its state and what a blocked screen waits on; on the
/// phone it also carries the run's Session menu, which desktop and terminal put in the head.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Bar {
    pub state: Option<(&'static str, Hue)>,
    pub lead: Option<String>,
    /// The run session whose Session menu shows; none for an id without a task.
    pub menu: Option<Id>,
}
/// Session header and selected surface. The Transcript | Terminal switch shows only where both
/// exist: a recorded pane or process, and a corrald that serves transcripts.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub sub: String,
    pub switch_key: &'static str,
    pub switch_hint: &'static str,
    pub phone_title: String,
    pub desktop_back_label: &'static str,
    pub back: Action,
    /// Whether the Transcript | Terminal switch shows.
    pub switch: bool,
    pub view: View,
    pub switch_labels: [&'static str; 2],
    pub switch_actions: [Action; 2],
    pub toggle: Action,
    /// Why the page shows what it does: the session left Agents, or its source is stale.
    pub notice: Option<(Hue, String)>,
    pub controls: Bar,
    pub transcript: Transcript,
    pub terminal: Option<Terminal>,
    pub reply: Reply,
    /// The pane's keys bar and type field under the terminal; none without a recorded session.
    pub pane: Option<PaneInput>,
    pub run_details: Run,
    /// The session's child agents and their messages, beside the run; none when it has neither.
    pub fleet: Option<Fleet>,
}

#[cfg(test)]
mod tests {
    use corral_types::{Role, SessionRef};

    use super::roster_tests::agent;
    use super::{Action, State, View};
    use crate::model::health::daemon_env;
    use crate::model::{Form, Id, Selection, daemon};

    /// A pi session opens on its transcript, any other run on its terminal; the switch then
    /// flips from what was shown.
    #[test]
    fn a_pi_session_opens_on_its_transcript_and_the_switch_flips_what_shows() {
        let mut feed = daemon::Feed::default();
        let mut pi = agent("T-1", "R1", Role::Worker);
        pi.run.session = Some(SessionRef::Pi {
            socket: "/run/s.sock".into(),
            inner: Box::new(SessionRef::Process {
                pid: 7,
                dir: "/runs/T-1/R1".into(),
            }),
            dir: "/runs/T-1/R1".into(),
        });
        feed.agents.insert(Id::from("T-1_R1"), pi);
        feed.agents
            .insert(Id::from("T-2_R1"), agent("T-2", "R1", Role::Worker));
        let mut state = State::default();
        assert_eq!(state.view_for(&feed, "T-1_R1"), View::Transcript);
        assert_eq!(state.view_for(&feed, "T-2_R1"), View::Terminal);
        let mut selection = Selection::default();
        selection.session = Some(Id::from("T-1_R1"));
        state.act_live(
            Action::ToggleView,
            &feed,
            &selection,
            daemon_env(&feed, Form::Desktop),
        );
        assert_eq!(state.view_for(&feed, "T-1_R1"), View::Terminal);
    }
}
