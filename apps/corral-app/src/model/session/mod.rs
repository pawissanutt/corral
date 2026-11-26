//! The one session page every session opens in: builds, assistant chats (`quick`, `A-n`),
//! worker, reviewer and planner runs, and a child agent's read-only conversation. The role and
//! what the host declares it serves (`SessionState.can`) decide the tabs, the dock and the
//! actions; every fact comes from the host's state, the served page or the session's row, and a
//! fact none of them reports is shown as absent. The per-session UI state (tab, draft, opened
//! rows, this device's sends and answers, each part's own state) lives here, keyed by session
//! id, for the app's lifetime. Every tab and panel of the page is registered here; each fills
//! its body in its own module.

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
mod board;
mod changes;
mod changes_diff;
mod choice;
mod composer;
mod dialog;
mod dock;
mod echo;
mod events;
mod exit;
mod files;
pub(crate) mod files_large;
mod fleet;
mod header;
mod keys;
mod land;
mod log;
mod notices;
mod plan;
mod plan_review;
mod proof;
mod questions;
mod queue;
mod read_state;
mod sidebar;
mod status;
mod tabs;
mod task;
pub mod transcript;
mod usage;

#[cfg(test)]
mod actions_tests;
#[cfg(test)]
mod board_tests;
#[cfg(test)]
mod changes_tests;
#[cfg(test)]
mod echo_tests;
#[cfg(test)]
mod exit_tests;
#[cfg(test)]
mod keys_tests;
#[cfg(test)]
mod picker_tests;
#[cfg(test)]
mod plan_review_progress_tests;
#[cfg(test)]
mod plan_review_tests;
#[cfg(test)]
mod plan_tests;
#[cfg(test)]
mod questions_tests;
#[cfg(test)]
mod queue_tests;
#[cfg(test)]
mod sidebar_tests;
#[cfg(test)]
mod tabs_tests;
#[cfg(test)]
mod task_tests;
#[cfg(test)]
mod usage_tests;

pub use act::Action;
pub use actions::{
    ActionGroup, ActionRow, CommandAction, PickRow, Picker, PickerAction, PickerKind, PickerUi,
    ScopeSwitch, SessionActions, SlashMenu, SlashRow, actions, model_picker, slash,
};
pub use activity::{ActivityLine, duration};
pub use board::{
    BoardAction, BoardControl, BoardRow, BoardTab, BoardUi, PlanCard, PlanTask, RecordLine,
    RecordPager, RecordSection, RecordView, board_tab,
};
pub use changes::{
    Author, AuthorChip, ChangesAction, ChangesTab, ChangesUi, ComparisonChip, ComparisonLine,
    FILTER_FIELD, FileRow, ScopeChip, Status, changes_tab,
};
pub use changes_diff::{FileDiff, HunkView, Pinned};
pub use choice::{ChoiceCard, ChoiceOption};
pub use composer::{
    ComposerTitle, EffortControl, HostBar, ModelPill, ReceiverChoice, ReceiverPill, SendButton,
    SendMode, SessionComposer, StopButton, composer, host_bar,
};
pub use dialog::{DecisionCard, DecisionKind, OPEN_GUARD_MS};
pub use dock::{
    AsideLine, Asked, CardConfirm, CardDraft, CardsRead, Compose, Confirming, Dock, DockAction,
    DockUi, Pending, dock,
};
pub use echo::{Echo, EchoState, Sent};
pub use events::{EventsAction, EventsUi, HostEvents, host_events};
pub use exit::{ExitOutcome, ExitProgress, ExitStep, ExitSummary, StepState, progress};
pub use files::{
    Crumb, FileEditor, FileIcon, FilesAction, FilesContent, FilesView, TreeItem, TreeRow,
};
pub use fleet::{
    AgentDetail, AgentRowView, AgentsTab, Asking, Clipped, ConfirmView, Conversation, EditLine,
    EditList, EndLine, FleetAction, FleetButton, FleetRequest, FleetUi, JobDetail, JobList,
    JobOutputView, JobRowView, MessageLine, MessageList, ModelChoiceRow, ModelPicker, Opened,
    Timeline, TimelineRow, agents_tab,
};
pub use header::{
    DecisionsChip, ExitStatus, Header, Meta, NeedsChip, StateWord, exit_status, header, role_word,
};
pub use keys::{KeyAction, KeyBinding, WEB_RESERVED, bound, keys, legend, legend_of, press};
pub use land::{CandidateView, GateLog};
pub use log::{SessionLog, ended_run, log};
pub use notices::{Notice, RecentNotices, Ring, recent};
pub use plan::{
    HistoryLine, Ledger, LedgerStep, PlanAction, PlanTab, PlanUi, StepRow,
    StepState as PlanStepState, ledger, plan_tab,
};
pub use plan_review::{PlanDoc, PlanReview, compact, compact_in_place};
pub use proof::{ProofAction, ProofTab, ProofUi, proof_tab};
pub use questions::{
    AnswerCard, AnswerTab, QuestionDock, QuestionLine, QuestionView, Review, ReviewRow,
    SendAnswers, TextRow,
};
pub use queue::{QueueControl, QueueDock, QueueItem};
pub use read_state::ReadState;
pub use sidebar::{
    ContextFact, ContextList, ContextTab, DetailSection, Details, FilesTab, McpRow, Panel,
    Section as DetailKind, SectionContent, SideAgent, SideFile, SideJob, SidebarAction, SidebarUi,
    brief as details_brief, details,
};
pub use status::{Alert, ContextMeter, Cost, RolePill, StatusBar, cost_text, quiet, spent, status};
pub use tabs::{PartBody, TabItem, tabs_for};
pub use task::{Attempt, TaskAction, TaskLink, TaskTab, TaskUi, task_tab};
pub use transcript::{Fill, Transcript};
pub use usage::{UsageAction, UsageLine, UsageTotalLine, UsageUi, UsageView, usage_view};

/// A session's role, as its page or row says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRole {
    Build,
    Code,
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
            "code" => Self::Code,
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
            Self::Code => "code".into(),
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
    Files,
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
            Self::Files => "files",
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
            Self::Files => "Files",
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
            Self::Files => "Alt+F",
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
    /// A background job's output.
    Job(String),
}

/// The surface that has the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Composer,
    Main,
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
    /// The dock: card answers being given, choices put aside, reason and answer modes, and the
    /// requests whose replies are read back.
    pub dock: DockUi,
    pub sidebar: SidebarUi,
    pub changes: ChangesUi,
    pub files: files::FilesUi,
    pub board: BoardUi,
    pub usage: UsageUi,
    pub task: TaskUi,
    pub proof: ProofUi,
    /// The Agents tab: the selected job, a confirmation, the model choices, opened texts and
    /// the requests whose replies are read back.
    pub fleet: FleetUi,
    /// The model and effort pickers and the `/` menu.
    pub picker: PickerUi,
    /// Bumped each time the transcript is asked back to its newest rows (End, Ctrl+End).
    pub jump: u32,
    /// Where the reader was in this scope's transcript when its last view closed.
    pub reading: transcript::Reading,
}

impl Ui {
    pub const EMPTY: Ui = Ui {
        tab: Tab::Chat,
        sub: None,
        focus: Focus::Composer,
        expanded: BTreeSet::new(),
        hide_thinking: false,
        draft: String::new(),
        to: None,
        agent: None,
        echoes: Vec::new(),
        notices: Ring::EMPTY,
        seen: None,
        dock: DockUi::EMPTY,
        sidebar: SidebarUi::EMPTY,
        changes: ChangesUi::EMPTY,
        files: files::FilesUi::EMPTY,
        board: BoardUi::EMPTY,
        usage: UsageUi::EMPTY,
        task: TaskUi::EMPTY,
        proof: ProofUi::EMPTY,
        fleet: FleetUi::EMPTY,
        picker: PickerUi::EMPTY,
        jump: 0,
        reading: transcript::Reading::End,
    };
}

impl Default for Ui {
    fn default() -> Self {
        Self::EMPTY
    }
}

static EMPTY_UI: Ui = Ui::EMPTY;

/// Every session page's UI state, by session id, and the session Ask is keyed to.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub ids: IdSupply,
    pub sessions: BTreeMap<Id, Ui>,
    /// The Details panel on this device, the same for every session page.
    pub panel: Panel,
    /// The concrete chat Ask shows (`A-n`), once a send to `quick` was acknowledged under it;
    /// none while Ask is on `quick` (before its first send, and again once that chat ended).
    pub ask: Option<Id>,
}

impl State {
    /// The session Ask reads and sends to: its chat once known, else `quick`.
    pub fn ask_id(&self) -> Id {
        self.ask
            .clone()
            .unwrap_or_else(|| Id::from(crate::model::assistant::QUICK))
    }
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
    if row.is_some_and(crate::model::agents::is_code) {
        return SessionRole::Code;
    }
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

    /// The session's served title, or the task's title; `untitled` when neither is available.
    pub fn title(&self) -> String {
        let named = self
            .row
            .map(|row| row.title.clone())
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

    /// `<provider>/<model>`: the host's current one, else the row's; for a new quick thread, the
    /// model the assistant is configured to answer with.
    pub fn model(&self) -> Option<String> {
        self.state
            .and_then(|state| state.model.clone())
            .or_else(|| self.row.and_then(|row| row.model.clone()))
            .or_else(|| self.run.and_then(|row| row.run.model.clone()))
            .or_else(|| {
                self.new_thread()
                    .then(|| crate::model::assistant::configured_model(self.feed))
                    .flatten()
            })
    }

    /// `quick` with no current chat: the next message starts one.
    pub fn new_thread(&self) -> bool {
        self.id.as_str() == crate::model::assistant::QUICK && self.row.is_none()
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
    tabs::items(cx).into_iter().map(|item| item.tab).collect()
}

/// What the main view shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Chat(Chat),
    /// `Chat › Usage`.
    Usage(UsageView),
    Agents(AgentsTab),
    Changes(ChangesTab),
    Files(FilesView),
    Board(BoardTab),
    /// The plan document of the plan review the owner opened.
    PlanReview(PlanDoc),
    Task(TaskTab),
    Proof(ProofTab),
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
    /// The build's exit after the owner chose it: the step rail, a repair round, the outcome.
    pub exit: Option<ExitProgress>,
    pub composer: Option<SessionComposer>,
    /// In the composer's place when the session takes no message now.
    pub bar: Option<HostBar>,
    pub status: StatusBar,
    /// The Details panel beside the conversation; none for a child or a log.
    pub details: Option<Details>,
    pub actions: SessionActions,
    /// The open model or effort list over the composer.
    pub picker: Option<Picker>,
    /// The `/` menu over the composer while the draft is a `/` command.
    pub slash: Option<SlashMenu>,
    /// Changes each time the transcript is asked back to its newest rows.
    pub jump: u32,
    /// The terminal key line for the focused surface.
    pub legend: Vec<(&'static str, &'static str)>,
    /// `! corrald unreachable · last update 14:02`.
    pub banner: Option<(Hue, String)>,
    /// Dispatched when the page mounts (and with `on: false` when it leaves): the live topic.
    /// None for a child transcript or a log, which have no live stream.
    pub watch: Option<Action>,
    /// Reads to make now: continuations of shown words, the waiting cards, the shown parts.
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
        SessionRole::Code => "This Code session works in the owner’s checkout.".into(),
        SessionRole::Assistant if cx.new_thread() => {
            "Ask the assistant about this workspace. Your first message starts a thread.".into()
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
    let empty =
        (cx.page.is_some() && transcript.messages.is_empty() && transcript.echoes.is_empty())
            .then(|| empty_chat(cx));
    Chat { transcript, empty }
}

/// The page of session `id` (a run `<task>_<run>`, a chat `quick` or `A-n`, a build `B-…`, a
/// child `<session>/child/<id>`).
pub fn page(feed: &Feed, state: &State, id: &Id, env: Env) -> SessionPage {
    let ui = state.ui(id);
    let facts = Facts::of(feed, id);
    let cx = Cx::new(feed, ui, id, env, &facts);
    let tabs = tabs::items(&cx);
    let tab = if tabs.iter().any(|item| item.tab == ui.tab) {
        ui.tab
    } else {
        Tab::Chat
    };
    let sub = ui.sub.clone().filter(|_| tab == ui.tab);
    let body = match (cx.log_only().then(|| log(id, feed)).flatten(), tab) {
        (_, Tab::Files) => Body::Files(files::view(&cx)),
        (Some(log), _) => Body::Log(log),
        (None, Tab::Chat) if sub == Some(SubView::Usage) => Body::Usage(usage_view(&cx)),
        (None, Tab::Agents) => Body::Agents(agents_tab(&cx, state)),
        (None, Tab::Changes) => Body::Changes(changes_tab(&cx)),
        (None, Tab::Board) => Body::Board(board_tab(&cx)),
        (None, Tab::Task) => Body::Task(task_tab(&cx)),
        (None, Tab::Proof) => Body::Proof(proof_tab(&cx)),
        (None, Tab::Chat) => match plan_review::doc(&cx) {
            Some(doc) => Body::PlanReview(doc),
            None => Body::Chat(chat(&cx)),
        },
    };
    let details = details(&cx, &state.panel);
    let mut fetches: Vec<Action> = transcript::fetches(feed, id, ui)
        .into_iter()
        .map(|token| Action::Fetch {
            session: id.clone(),
            token,
        })
        .collect();
    fetches.extend(dock::fetch(&cx));
    fetches.extend(dock::probe(&cx));
    fetches.extend(actions::fetches(&cx));
    fetches.extend(board::record_fetches(&cx));
    fetches.extend(match &body {
        Body::Usage(_) => usage::fetches(&cx),
        Body::Changes(_) => changes::fetches(&cx),
        Body::Files(_) => files::fetches(&cx),
        Body::Board(_) => board::fetches(&cx),
        Body::Task(_) => task::fetches(&cx),
        Body::Proof(_) => proof::fetches(&cx),
        Body::Agents(_) => fleet::fetches(&cx, state),
        Body::PlanReview(_) => plan_review::fetches(&cx).into_iter().collect(),
        Body::Chat(_) | Body::Log(_) => Vec::new(),
    });
    if details.is_some() {
        fetches.extend(sidebar::fetches(&cx, &state.panel));
    }
    let streams = *cx.role != SessionRole::Child && !cx.log_only() && feed.capable(SESSION_V1);
    let concrete = cx
        .page
        .map(|page| page.id.clone())
        .or_else(|| cx.row.map(|row| row.id.clone()));
    let catalog = actions(&cx);
    let slash = slash(&cx, &catalog);
    SessionPage {
        id: id.clone(),
        concrete,
        role: facts.role.clone(),
        host: cx.host,
        header: header(&cx),
        sub,
        focus: ui.focus,
        legend: legend_of(&cx, tab, slash.is_some()),
        dock: dock(&cx),
        exit: progress(&cx),
        composer: composer(&cx),
        bar: host_bar(&cx),
        status: status(&cx),
        details,
        picker: model_picker(&cx),
        slash,
        jump: ui.jump,
        actions: catalog,
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
        fetches,
        tabs,
        tab,
        body,
    }
}
