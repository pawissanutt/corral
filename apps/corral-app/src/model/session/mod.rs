//! The one session page every session opens in: builds, assistant chats (`quick`, `A-n`),
//! worker, reviewer and planner runs, and a child agent's read-only conversation. The role and
//! what the host declares it serves (`SessionState.can`) decide the tabs, the dock and the
//! actions; every fact comes from the host's state, the served page or the session's row, and a
//! fact none of them reports is shown as absent. The per-session UI state (tab, draft, opened
//! rows, this device's sends) lives here, keyed by session id, for the app's lifetime.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::app::SESSION_V1;
use corral_protocol::detail::{DetailRef, HostPresence, SessionPage as Served};
use corral_protocol::rows::{AgentRow, AssistantKind, AssistantRow, AssistantState, BuildFacts};
use corral_protocol::session_state::{ExitBrief, SessionState};
use corral_types::{Run, SessionStatus};

use crate::model::Id;
use crate::model::agents::Control;
use crate::model::daemon::{DetailCache, Feed, Link};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::id::IdSupply;
use crate::model::work::session_task;

mod act;
mod actions;
mod activity;
mod composer;
mod dock;
mod echo;
mod fleet;
mod header;
mod keys;
mod log;
mod notices;
mod read_state;
mod status;
mod tabs;
pub mod transcript;

#[cfg(test)]
mod echo_tests;
#[cfg(test)]
mod tabs_tests;

pub use act::Action;
pub use actions::{ActionGroup, ActionRow, SessionActions, actions};
pub use activity::{ActivityLine, duration};
pub use composer::{
    ComposerTitle, EffortControl, HostBar, ModelPill, ReceiverChoice, ReceiverPill, SendButton,
    SendMode, SessionComposer, StopButton, composer, host_bar,
};
pub use dock::{Dock, DockCard, QueueLine, QueueSummary, dock};
pub use echo::{Echo, EchoState, Sent};
pub use fleet::{
    AgentDetail, AgentRowView, AgentsTab, MessageLine, MessageList, Timeline, TimelineRow,
    agents_tab,
};
pub use header::{DecisionsChip, ExitStatus, Header, NeedsChip, StateWord, exit_status, header};
pub use keys::{KeyAction, KeyBinding, bound, keys, legend, press};
pub use log::{SessionLog, ended_run, log};
pub use notices::{Notice, RecentNotices, Ring, recent};
pub use read_state::ReadState;
pub use status::{Alert, ContextMeter, Cost, RolePill, StatusBar, cost_text, status};
pub use tabs::{Badge, TabItem, tabs_for};
pub use transcript::{Fill, Transcript};

/// The tabs whose bodies this build of the page draws.
pub const BUILT: &[Tab] = &[Tab::Chat, Tab::Agents];

/// A session's role, as its page or row says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRole {
    Build,
    /// The app's one chat agent (the PM and the assistant are the same agent).
    Assistant,
    Worker,
    Reviewer,
    Planner,
    PlanReviewer,
    /// A child agent's transcript, read-only.
    Child,
    Other(String),
}

impl SessionRole {
    pub fn of(role: &str) -> Self {
        match role {
            "build" => Self::Build,
            "assistant" | "chat" | "pm" => Self::Assistant,
            "worker" => Self::Worker,
            "reviewer" => Self::Reviewer,
            "planner" => Self::Planner,
            "plan_reviewer" => Self::PlanReviewer,
            "child" => Self::Child,
            other => Self::Other(other.into()),
        }
    }

    /// Whether the session runs for one task: it has Task and Proof tabs and a decisions chip.
    pub fn task_bound(&self) -> bool {
        matches!(
            self,
            Self::Worker | Self::Reviewer | Self::Planner | Self::PlanReviewer
        )
    }

    pub fn label(&self) -> String {
        match self {
            Self::Build => "build".into(),
            Self::Assistant => "assistant".into(),
            Self::Worker => "worker".into(),
            Self::Reviewer => "reviewer".into(),
            Self::Planner => "planner".into(),
            Self::PlanReviewer => "plan reviewer".into(),
            Self::Child => "child".into(),
            Self::Other(role) => role.replace('_', " "),
        }
    }
}

/// A main-view tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum Tab {
    Task,
    #[default]
    Chat,
    Agents,
    Changes,
    Board,
    Proof,
}

impl Tab {
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Chat => "chat",
            Self::Agents => "agents",
            Self::Changes => "changes",
            Self::Board => "board",
            Self::Proof => "proof",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Task => "Task",
            Self::Chat => "Chat",
            Self::Agents => "Agents",
            Self::Changes => "Changes",
            Self::Board => "Board",
            Self::Proof => "Proof",
        }
    }

    /// The terminal UI's key for the tab.
    pub fn key(self, _role: &SessionRole) -> &'static str {
        match self {
            Self::Task => "F5",
            Self::Chat => "F9",
            Self::Agents => "F8",
            Self::Changes => "F7",
            Self::Board | Self::Proof => "F10",
        }
    }
}

/// A view inside a tab, named on the tab: `Chat › Usage`, `Agents › a2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubView {
    Usage,
    /// A child agent's conversation.
    Agent(String),
}

/// The surface that has the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Composer,
    Main,
    Sidebar,
    Queue,
    Questions,
    Dock,
}

/// One session page's UI state, kept across resizes, tab switches and leaving the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ui {
    pub tab: Tab,
    pub sub: Option<SubView>,
    pub focus: Focus,
    /// Transcript rows the owner opened, by stable item key. A thinking block shows by default,
    /// so its key here folds it instead.
    pub expanded: BTreeSet<String>,
    /// An earlier page of the transcript shows rather than the latest messages.
    pub earlier: bool,
    /// Every thinking block is hidden (`Alt+H`).
    pub hide_thinking: bool,
    pub draft: String,
    /// The child the next message goes to; none for the session's own agent.
    pub to: Option<String>,
    /// The Agents tab's selected child.
    pub agent: Option<String>,
    /// This device's sends not yet retired, in send order.
    pub echoes: Vec<Sent>,
    pub notices: Ring,
    /// The transcript's row count when Chat was last left or opened; none before.
    pub seen: Option<usize>,
}

impl Ui {
    pub const EMPTY: Ui = Ui {
        tab: Tab::Chat,
        sub: None,
        focus: Focus::Composer,
        expanded: BTreeSet::new(),
        earlier: false,
        hide_thinking: false,
        draft: String::new(),
        to: None,
        agent: None,
        echoes: Vec::new(),
        notices: Ring::EMPTY,
        seen: None,
    };
}

impl Default for Ui {
    fn default() -> Self {
        Self::EMPTY
    }
}

static EMPTY_UI: Ui = Ui::EMPTY;

/// Every session page's UI state, by session id.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub ids: IdSupply,
    pub sessions: BTreeMap<Id, Ui>,
}

/// Whether the session's host runs, as the served page or the session's row says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    Running,
    /// Stopped; the transcript is kept and the host can be started again.
    NotRunning,
    /// corrald cannot reach it; the last state is retained.
    Unreachable,
    /// The session ended for good.
    Ended,
    /// Nothing says yet.
    Unknown,
}

/// Whether the host serves a part or an operation, by its declared capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Can {
    Yes,
    /// Served, but not now, for this reason.
    Unavailable(String),
    /// Not served by this session: nothing is drawn for it.
    Absent,
    /// The host declared nothing (no state yet, or a host that does not declare).
    Unknown,
}

/// Everything one session's view builders read.
#[derive(Debug, Clone, Copy)]
pub struct Cx<'a> {
    pub id: &'a Id,
    pub feed: &'a Feed,
    pub ui: &'a Ui,
    pub env: Env,
    pub role: &'a SessionRole,
    pub cache: Option<&'a DetailCache>,
    pub page: Option<&'a Served>,
    pub live: Option<&'a transcript::LiveSession>,
    pub state: Option<&'a SessionState>,
    /// The chat or build row.
    pub row: Option<&'a AssistantRow>,
    /// The live run's row.
    pub run: Option<&'a AgentRow>,
    /// The run of an ended headless session, read from its task's record.
    pub ended: Option<&'a Run>,
    pub host: Host,
    pub task: Option<&'a Id>,
}

/// The facts a page context is made from, owned so the context can borrow them.
pub struct Facts {
    pub role: SessionRole,
    pub task: Option<Id>,
}

impl Facts {
    pub fn of(feed: &Feed, id: &Id) -> Self {
        let cache = feed.detail(&DetailRef::Session { id: id.to_string() });
        let page = transcript::page(cache);
        let row = crate::model::assistant::row(feed, id.as_str());
        let run = feed.agents.get(id.as_str());
        let ended = run
            .is_none()
            .then(|| ended_run(feed, id.as_str()))
            .flatten();
        let task = run
            .map(|row| row.run.task.clone())
            .or_else(|| ended.map(|run| run.task.clone()))
            .or_else(|| page.and_then(|page| page.task.clone()))
            .or_else(|| session_task(id).map(str::to_owned))
            .map(|task| Id::from(task.as_str()));
        Self {
            role: role_of(id.as_str(), page, row, run.map(|row| &row.run).or(ended)),
            task,
        }
    }
}

fn role_of(
    id: &str,
    page: Option<&Served>,
    row: Option<&AssistantRow>,
    run: Option<&Run>,
) -> SessionRole {
    if let Some(page) = page {
        return SessionRole::of(&page.role);
    }
    if id.contains("/child/") {
        return SessionRole::Child;
    }
    if let Some(row) = row {
        return match row.kind {
            AssistantKind::Build => SessionRole::Build,
            _ => SessionRole::Assistant,
        };
    }
    if id.starts_with("B-") {
        return SessionRole::Build;
    }
    if id == crate::model::assistant::QUICK || id.starts_with("A-") {
        return SessionRole::Assistant;
    }
    if let Some(run) = run {
        return SessionRole::of(run.role.as_str());
    }
    if id.ends_with("/reviewer") {
        return SessionRole::Reviewer;
    }
    if session_task(id).is_some() {
        return SessionRole::Worker;
    }
    SessionRole::Other("agent".into())
}

fn host_of(
    page: Option<&Served>,
    row: Option<&AssistantRow>,
    run: Option<&AgentRow>,
    ended: Option<&Run>,
) -> Host {
    if ended.is_some()
        || row.is_some_and(|row| row.state == AssistantState::Ended)
        || run.is_some_and(|run| run.run.ended.is_some())
    {
        return Host::Ended;
    }
    // A run whose harness speaks no Pi socket has no host the page could report.
    if let Some(run) = run.filter(|run| log::without_pi(run)) {
        return match run.status {
            SessionStatus::Gone => Host::NotRunning,
            _ => Host::Running,
        };
    }
    if let Some(page) = page {
        return match page.host {
            HostPresence::Running => Host::Running,
            HostPresence::NotRunning => Host::NotRunning,
            HostPresence::Unreachable => Host::Unreachable,
            HostPresence::Other => Host::Unknown,
        };
    }
    if let Some(row) = row {
        return match row.state {
            AssistantState::Cold => Host::NotRunning,
            _ => Host::Running,
        };
    }
    match run.map(|run| &run.status) {
        Some(SessionStatus::Gone) => Host::NotRunning,
        Some(_) => Host::Running,
        None => Host::Unknown,
    }
}

impl<'a> Cx<'a> {
    pub fn new(feed: &'a Feed, ui: &'a Ui, id: &'a Id, env: Env, facts: &'a Facts) -> Self {
        let cache = feed.detail(&DetailRef::Session { id: id.to_string() });
        let page = transcript::page(cache);
        let live = feed.live.get(id.as_str());
        let row = crate::model::assistant::row(feed, id.as_str());
        let run = feed.agents.get(id.as_str());
        let ended = run
            .is_none()
            .then(|| ended_run(feed, id.as_str()))
            .flatten();
        Self {
            id,
            feed,
            ui,
            env,
            role: &facts.role,
            cache,
            page,
            live,
            state: live.and_then(|live| live.state.as_ref()),
            row,
            run,
            ended,
            host: host_of(page, row, run, ended),
            task: facts.task.as_ref(),
        }
    }

    /// Whether the host serves capability `id`, as its state declares.
    pub fn can(&self, id: &str) -> Can {
        let Some(state) = self.state.filter(|state| !state.can.is_empty()) else {
            return Can::Unknown;
        };
        match state.can.iter().find(|capability| capability.id == id) {
            None => Can::Absent,
            Some(capability) => match &capability.unavailable {
                Some(why) => Can::Unavailable(why.clone()),
                None => Can::Yes,
            },
        }
    }

    pub fn build(&self) -> Option<&'a BuildFacts> {
        self.row.and_then(|row| row.build.as_ref())
    }

    /// The build's exit brief: the host's current one, else the record's.
    pub fn exit(&self) -> Option<&'a ExitBrief> {
        self.state
            .and_then(|state| state.exit.as_ref())
            .or_else(|| self.build().and_then(|build| build.exit.as_ref()))
    }

    /// The build's record says its host runs in a terminal this daemon cannot reach.
    pub fn elsewhere(&self) -> bool {
        self.row.is_some_and(header::open_elsewhere)
    }

    /// A session without a Pi host: its page is its read-only log.
    pub fn log_only(&self) -> bool {
        self.ended.is_some() || self.run.is_some_and(log::without_pi)
    }

    /// The session's name: the build's, the chat's title, the task's title; `untitled` when the
    /// host recorded none.
    pub fn title(&self) -> String {
        let named = self
            .build()
            .map(|build| build.name.clone())
            .or_else(|| self.row.map(|row| row.title.clone()))
            .or_else(|| {
                self.task
                    .and_then(|task| self.feed.board.task(task))
                    .map(|task| task.title().to_owned())
            })
            .filter(|name| !name.trim().is_empty());
        named.unwrap_or_else(|| "untitled".into())
    }

    pub fn run_id(&self) -> Option<String> {
        self.run
            .map(|row| row.run.id.clone())
            .or_else(|| self.ended.map(|run| run.id.clone()))
            .or_else(|| self.page.and_then(|page| page.run.clone()))
    }

    /// The id as the roster shows it: `B-4`, `A-2`, `T-12 R2`.
    pub fn shown_id(&self) -> String {
        if let Some(row) = self.row {
            return row.id.clone();
        }
        match (self.task, self.run_id()) {
            (Some(task), Some(run)) => format!("{task} {run}"),
            _ => self.id.to_string(),
        }
    }

    /// `<provider>/<model>`: the host's current one, else the row's.
    pub fn model(&self) -> Option<String> {
        self.state
            .and_then(|state| state.model.clone())
            .or_else(|| self.row.and_then(|row| row.model.clone()))
            .or_else(|| self.run.and_then(|row| row.run.model.clone()))
    }

    pub fn effort(&self) -> Option<String> {
        self.state
            .and_then(|state| state.effort.clone())
            .or_else(|| self.row.and_then(|row| row.effort.clone()))
            .or_else(|| self.run.and_then(|row| row.run.effort.clone()))
    }

    /// The run controls that apply now.
    pub fn controls(&self) -> Vec<Control> {
        crate::model::agents::controls(self.feed, self.id.as_str())
    }
}

/// The tabs the page of `cx` shows, in order.
pub fn shown_tabs(cx: &Cx<'_>) -> Vec<Tab> {
    if cx.log_only() {
        return Vec::new();
    }
    tabs::items(cx, BUILT)
        .into_iter()
        .map(|item| item.tab)
        .collect()
}

/// What the main view shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Chat(Chat),
    Agents(AgentsTab),
    /// A session without a Pi host: its read-only log.
    Log(SessionLog),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chat {
    pub transcript: Transcript,
    /// Before the first row: who receives the message (§3.5).
    pub empty: Option<EmptyChat>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptyChat {
    pub sentence: String,
    /// `⏎ send   F1 keys   F2 actions`; the phone shows none.
    pub keys: Vec<(&'static str, &'static str)>,
}

/// The session page.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionPage {
    /// The scope the page reads (`quick` included).
    pub id: Id,
    /// The concrete session (`A-7` for `quick`), once served.
    pub concrete: Option<String>,
    pub role: SessionRole,
    pub host: Host,
    pub header: Header,
    pub tabs: Vec<TabItem>,
    pub tab: Tab,
    pub sub: Option<SubView>,
    pub focus: Focus,
    pub body: Body,
    pub dock: Dock,
    pub composer: Option<SessionComposer>,
    /// In the composer's place when the session takes no message now.
    pub bar: Option<HostBar>,
    pub status: StatusBar,
    pub actions: SessionActions,
    /// The terminal key line for the focused surface.
    pub legend: Vec<(&'static str, &'static str)>,
    /// `! corrald unreachable · last update 14:02`.
    pub banner: Option<(Hue, String)>,
    /// Dispatched when the page mounts (and with `on: false` when it leaves): the live topic.
    /// None for a child transcript or a log, which have no live stream.
    pub watch: Option<Action>,
    /// Continuations of shown words to fetch now.
    pub fetches: Vec<Action>,
}

fn empty_chat(cx: &Cx<'_>) -> EmptyChat {
    let sentence = match cx.role {
        SessionRole::Build => {
            let (worktree, branch) = cx.build().map_or_else(
                || ("its worktree".to_owned(), "its branch".to_owned()),
                |build| {
                    (
                        build.worktree.clone(),
                        build.branch.clone().unwrap_or_else(|| "its branch".into()),
                    )
                },
            );
            format!(
                "Build {} works in {worktree} on {branch}. What you send goes to the parent agent, which can start child agents. Choose Finish to keep, land or keep iterating.",
                cx.title()
            )
        }
        SessionRole::Assistant => format!(
            "Assistant {}. What you send goes to this assistant.",
            cx.shown_id()
        ),
        SessionRole::Child => "This agent has recorded nothing yet.".into(),
        _ => "Waiting for corral's first instruction. What you send goes to this agent.".into(),
    };
    EmptyChat {
        sentence,
        keys: if cx.env.form == crate::model::nav::Form::Phone {
            Vec::new()
        } else {
            vec![("⏎", "send"), ("Ctrl+K", "actions")]
        },
    }
}

fn chat(cx: &Cx<'_>) -> Chat {
    let transcript = transcript::transcript(cx.id, cx.ui, cx.cache, cx.feed);
    let empty = (cx.page.is_some()
        && transcript.messages.is_empty()
        && transcript.echoes.is_empty()
        && transcript.live.is_empty())
    .then(|| empty_chat(cx));
    Chat { transcript, empty }
}

/// The page of session `id` (a run `<task>_<run>`, a chat `quick` or `A-n`, a build `B-…`, a
/// child `<session>/child/<id>`).
pub fn page(feed: &Feed, state: &State, id: &Id, env: Env) -> SessionPage {
    let ui = state.ui(id);
    let facts = Facts::of(feed, id);
    let cx = Cx::new(feed, ui, id, env, &facts);
    let tabs = if cx.log_only() {
        Vec::new()
    } else {
        tabs::items(&cx, BUILT)
    };
    let tab = if tabs.iter().any(|item| item.tab == ui.tab) {
        ui.tab
    } else {
        Tab::Chat
    };
    let body = match (cx.log_only().then(|| log(id, feed)).flatten(), tab) {
        (Some(log), _) => Body::Log(log),
        (None, Tab::Agents) => Body::Agents(agents_tab(&cx)),
        (None, _) => Body::Chat(chat(&cx)),
    };
    let streams = *cx.role != SessionRole::Child && !cx.log_only() && feed.capable(SESSION_V1);
    let concrete = cx
        .page
        .map(|page| page.id.clone())
        .or_else(|| cx.row.map(|row| row.id.clone()));
    let working = composer::working(&cx);
    SessionPage {
        id: id.clone(),
        concrete,
        role: facts.role.clone(),
        host: cx.host,
        header: header(&cx),
        sub: ui.sub.clone().filter(|_| tab == ui.tab),
        focus: ui.focus,
        legend: legend(ui.focus, tab, working),
        dock: dock(&cx),
        composer: composer(&cx),
        bar: host_bar(&cx),
        status: status(&cx),
        actions: actions(&cx),
        banner: (feed.link != Link::Up).then(|| {
            (
                Hue::Held,
                format!(
                    "! corrald unreachable · last update {}",
                    feed.synced.as_deref().unwrap_or("unknown")
                ),
            )
        }),
        watch: streams.then(|| Action::Watch {
            session: id.clone(),
            on: true,
        }),
        fetches: transcript::fetches(feed, id)
            .into_iter()
            .map(|token| Action::Fetch {
                session: id.clone(),
                token,
            })
            .collect(),
        tabs,
        tab,
        body,
    }
}
