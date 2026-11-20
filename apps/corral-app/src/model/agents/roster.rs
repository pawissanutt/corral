//! One roster of every session the daemon addresses, of every role: corrald's live runs, the
//! assistant's chats and the build sessions, one row each. Live shows hosts that run or wait;
//! All adds sessions whose host is not running (resumable builds and chats, ended ones). Needs
//! you first, then working, idle, not running and ended; newest activity first within each, the
//! pinned quick chat first among its equals. A row opens its session page.

use corral_protocol::rows::{
    AgentRow, AssistantKind, AssistantLocation, AssistantRow, AssistantState,
};
use corral_types::SessionStatus;

use crate::model::board::Mark;
use crate::model::daemon;
use crate::model::glance::{Stat, StatIcon};
use crate::model::projects::ProjectFilter;
use crate::model::session::{Fill, cost_text, exit_status};
use crate::model::{Hue, Id, assistant};

use super::start::{NewMenu, new_menu};
use super::{Action, State, live};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RosterRole {
    #[default]
    All,
    Build,
    Assistant,
    Worker,
    Reviewer,
    Other(String),
}

impl RosterRole {
    pub fn of(role: &str) -> Self {
        match role {
            "build" => Self::Build,
            "assistant" => Self::Assistant,
            "worker" => Self::Worker,
            "reviewer" => Self::Reviewer,
            other => Self::Other(other.into()),
        }
    }
    pub fn slug(&self) -> &str {
        match self {
            Self::All => "all",
            Self::Build => "build",
            Self::Assistant => "assistant",
            Self::Worker => "worker",
            Self::Reviewer => "reviewer",
            Self::Other(role) => role,
        }
    }
    fn label(&self) -> String {
        match self {
            Self::All => "All".into(),
            Self::Build => "Build".into(),
            Self::Assistant => "Assistant".into(),
            Self::Worker => "Workers".into(),
            Self::Reviewer => "Reviewers".into(),
            Self::Other(role) => live::role_label(role),
        }
    }
}

/// Which sessions the roster lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Show {
    /// Hosts that run or wait, and the pinned quick chat.
    #[default]
    Live,
    /// Also sessions whose host is not running, and ended ones the daemon lists.
    All,
}

impl Show {
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::All => "all",
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::All => "All",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleTab {
    pub id: String,
    pub label: String,
    pub count: usize,
    pub selected: bool,
    pub action: Action,
}

/// `seg-show-<live|all>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowTab {
    pub id: String,
    pub label: &'static str,
    pub selected: bool,
    pub action: Action,
}

/// A row's state glyph and word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowState {
    pub spinner: bool,
    /// `▲ ! ○ ✓ ✗`, when not a spinner.
    pub glyph: Option<&'static str>,
    /// `Working`, `Needs you`, `Idle`, `Not running`, `Ended`, or a build's exit status.
    pub word: String,
    pub hue: Hue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterRow {
    /// `agent-<session>`.
    pub id: String,
    pub session: Id,
    /// The run's task, whose Actions menu the row carries; none for a chat or a build.
    pub task: Option<Id>,
    pub state: RowState,
    pub role: String,
    pub shown_id: String,
    pub name: String,
    /// False when the host recorded no name: `untitled` is drawn muted.
    pub named: bool,
    /// `corral`, `corral · build/auth`, `workspace`, `–`.
    pub project: String,
    pub doing: String,
    pub doing_hint: String,
    /// `▲ 2` when the session or its children wait on the owner.
    pub needs: Option<String>,
    /// `<provider>/<model> · effort`, `–`.
    pub agent: String,
    /// `$1.24`, `$–`.
    pub cost: String,
    /// `24%`, `–`.
    pub ctx: String,
    /// `quiet 12m`, `–`.
    pub quiet: String,
    pub since: String,
    pub pinned: bool,
    /// Its host is stopped and can be started again from its page.
    pub resumable: bool,
    /// `Remove…`: a stopped build's worktree, branch and record, after its dry run.
    pub remove: Option<Action>,
    /// Opens the session page.
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roster {
    pub title: &'static str,
    pub stats: Vec<Stat>,
    pub roles: Vec<RoleTab>,
    pub show: Vec<ShowTab>,
    pub new: NewMenu,
    pub rows: Vec<RosterRow>,
    pub columns: [&'static str; 11],
    pub fill: Fill,
    pub stale: Option<String>,
}

/// A roster line's source: a live run (keyed `<task>_<run>`), or a chat or build row.
#[derive(Clone, Copy)]
pub(super) struct Source<'a> {
    pub id: &'a str,
    pub run: Option<&'a AgentRow>,
    pub chat: Option<&'a AssistantRow>,
}

/// Group order: needs you, working, idle, not running, ended.
const NEEDS: u8 = 0;
const WORKING: u8 = 1;
const IDLE: u8 = 2;
const STOPPED: u8 = 3;
const ENDED: u8 = 4;

fn state(
    spinner: bool,
    glyph: Option<&'static str>,
    word: impl Into<String>,
    hue: Hue,
) -> RowState {
    RowState {
        spinner,
        glyph,
        word: word.into(),
        hue,
    }
}

impl<'a> Source<'a> {
    pub fn role(self) -> &'a str {
        match (self.run, self.chat) {
            (Some(run), _) => run.run.role.as_str(),
            (None, Some(chat)) if chat.kind == AssistantKind::Build => "build",
            _ => "assistant",
        }
    }
    fn task(self, feed: &'a daemon::Feed) -> Option<&'a daemon::Task> {
        self.run.and_then(|row| feed.board.task(&row.run.task))
    }
    /// The project the session works in: a run's task's, a build's own; none for a chat.
    pub fn project(self, feed: &'a daemon::Feed) -> Option<&'a str> {
        match (self.run, self.chat) {
            (Some(_), _) => self.task(feed).and_then(|task| task.project()),
            (None, Some(chat)) => chat
                .build
                .as_ref()
                .and_then(|build| build.project.as_deref()),
            (None, None) => None,
        }
    }
    pub fn pinned(self, feed: &daemon::Feed) -> bool {
        self.chat
            .is_some_and(|chat| chat.kind == AssistantKind::Quick)
            && assistant::quick_id(feed).as_deref() == Some(self.id)
    }
    /// The build's record says its host runs in a terminal this daemon cannot reach.
    fn elsewhere(self) -> bool {
        self.chat.is_some_and(|chat| {
            chat.state == AssistantState::Cold && chat.live_in == AssistantLocation::Pane
        })
    }
    /// Stopped and startable again from its page.
    pub fn resumable(self) -> bool {
        match (self.run, self.chat) {
            (Some(run), _) => matches!(run.status, SessionStatus::Gone),
            (None, Some(chat)) => {
                matches!(chat.state, AssistantState::Cold | AssistantState::Failed)
                    && !self.elsewhere()
            }
            (None, None) => false,
        }
    }
    /// A build whose host is stopped here: its worktree, branch and record can be removed.
    pub(super) fn removable(self) -> bool {
        self.chat.is_some_and(|chat| {
            chat.kind == AssistantKind::Build
                && matches!(chat.state, AssistantState::Cold | AssistantState::Ended)
        }) && !self.elsewhere()
    }
    /// The row's state and its group.
    pub fn state(self) -> (RowState, u8) {
        if let Some(run) = self.run {
            return match &run.status {
                SessionStatus::Working => (state(true, None, "Working", Hue::Worker), WORKING),
                SessionStatus::Blocked { .. } => {
                    (state(false, Some("▲"), "Needs you", Hue::You), NEEDS)
                }
                SessionStatus::Idle if run.paused == Some(true) => {
                    (state(false, Some("○"), "Paused", Hue::Held), IDLE)
                }
                SessionStatus::Idle => (state(false, None, "Idle", Hue::Neutral), IDLE),
                SessionStatus::Gone => (
                    state(false, Some("○"), "Not running", Hue::Neutral),
                    STOPPED,
                ),
            };
        }
        let Some(chat) = self.chat else {
            return (state(false, None, "state unknown", Hue::Neutral), IDLE);
        };
        let exit = chat
            .build
            .as_ref()
            .and_then(|build| build.exit.as_ref())
            .and_then(exit_status);
        if let Some(exit) = exit.filter(|_| chat.state != AssistantState::Cold) {
            let group = match exit.hue {
                Hue::Held => NEEDS,
                Hue::Corral => WORKING,
                _ => IDLE,
            };
            let glyph = match exit.hue {
                Hue::Corral => None,
                Hue::Fail => Some("✗"),
                Hue::Ok => Some("✓"),
                _ => Some("●"),
            };
            return (
                state(exit.hue == Hue::Corral, glyph, exit.short, exit.hue),
                group,
            );
        }
        match chat.state {
            AssistantState::Working | AssistantState::Starting => {
                (state(true, None, "Working", Hue::Worker), WORKING)
            }
            AssistantState::Waiting => (state(false, Some("▲"), "Needs you", Hue::You), NEEDS),
            AssistantState::Stopping => (state(true, None, "Stopping", Hue::Neutral), WORKING),
            AssistantState::Paused => (state(false, Some("○"), "Paused", Hue::Held), IDLE),
            AssistantState::Failed => (state(false, Some("✗"), "Failed", Hue::Fail), IDLE),
            AssistantState::Cold if self.elsewhere() => (
                state(false, Some("○"), "Open in another terminal", Hue::Neutral),
                STOPPED,
            ),
            AssistantState::Cold => (
                state(false, Some("○"), "Not running", Hue::Neutral),
                STOPPED,
            ),
            AssistantState::Ended => (state(false, None, "Ended", Hue::Neutral), ENDED),
            AssistantState::Ready | AssistantState::Other => {
                (state(false, None, "Idle", Hue::Neutral), IDLE)
            }
        }
    }
    /// Whether the host runs or waits: what Live lists.
    pub fn live(self) -> bool {
        self.state().1 < STOPPED
    }
    pub fn name(self, feed: &daemon::Feed) -> Option<String> {
        let named = match (self.run, self.chat) {
            (Some(_), _) => self.task(feed).map(|task| task.title().to_owned()),
            (None, Some(chat)) => Some(
                chat.build
                    .as_ref()
                    .map_or_else(|| chat.title.clone(), |build| build.name.clone()),
            ),
            (None, None) => None,
        };
        named.filter(|name| !name.trim().is_empty())
    }
    /// What the session is doing: a blocked screen's prompt, the task's progress while the
    /// run works; a chat's or build's waiting question, or how many agents it runs.
    pub fn doing(self, feed: &daemon::Feed) -> Option<String> {
        if let Some(row) = self.run {
            let task = self.task(feed);
            let said = match &row.status {
                SessionStatus::Blocked { .. } => live::excerpt(row)
                    .map(str::to_owned)
                    .or_else(|| task.map(daemon::Task::now)),
                SessionStatus::Working => task.map(daemon::Task::now),
                SessionStatus::Idle if row.paused == Some(true) => {
                    Some("paused · its input waits for Resume".into())
                }
                SessionStatus::Idle => task
                    .and_then(daemon::Task::blocked)
                    .map(|b| b.reason.clone()),
                SessionStatus::Gone => None,
            };
            return said.filter(|text| !text.trim().is_empty());
        }
        let chat = self.chat?;
        if let Some(wait) = chat
            .wait
            .as_ref()
            .filter(|_| chat.state == AssistantState::Waiting)
        {
            return wait.text.lines().next().map(str::to_owned);
        }
        let live = chat.build.as_ref().map_or(0, |build| build.children.live);
        (live > 0).then(|| {
            format!(
                "{live} {} running",
                if live == 1 { "agent" } else { "agents" }
            )
        })
    }
    /// `▲ n`: the session and its children waiting on the owner.
    pub fn needs(self) -> Option<String> {
        let waiting = u32::from(
            self.run
                .is_some_and(|run| matches!(run.status, SessionStatus::Blocked { .. }))
                || self
                    .chat
                    .is_some_and(|chat| chat.state == AssistantState::Waiting),
        );
        let children = self
            .chat
            .and_then(|chat| chat.build.as_ref())
            .map_or(0, |build| build.children.waiting);
        let count = waiting.max(children);
        (count > 0).then(|| format!("▲ {count}"))
    }
    /// The model and effort: the row's model; a chat without one names its profile.
    pub fn agent(self) -> Option<String> {
        let (model, effort) = match (self.run, self.chat) {
            (Some(run), _) => (run.run.model.clone(), run.run.effort.clone()),
            (None, Some(chat)) => (
                chat.model.clone().or_else(|| chat.profile.clone()),
                chat.effort.clone(),
            ),
            (None, None) => (None, None),
        };
        let model = model?;
        Some(match effort {
            Some(effort) => format!("{model} · {effort}"),
            None => model,
        })
    }
    /// Unix seconds of its newest activity, for the order within a group.
    fn activity(self) -> u64 {
        match (self.run, self.chat) {
            (Some(run), _) => run
                .run
                .quiet_since
                .unwrap_or(run.run.started)
                .as_second()
                .try_into()
                .unwrap_or(0),
            (None, Some(chat)) => chat
                .updated
                .as_deref()
                .or(chat.created.as_deref())
                .and_then(live::unix)
                .unwrap_or(0),
            (None, None) => 0,
        }
    }
    pub fn is_stale(self, feed: &daemon::Feed) -> bool {
        live::stale(
            feed,
            if self.run.is_some() {
                "agents"
            } else {
                "assistant"
            },
        ) || self
            .task(feed)
            .is_some_and(|task| task.rail.iter().any(|step| step.mark == Mark::Stale))
    }
}

fn in_scope(source: Source<'_>, feed: &daemon::Feed, filter: &ProjectFilter) -> bool {
    match (source.run, source.chat) {
        (Some(row), _) => feed
            .board
            .task(&row.run.task)
            .map_or(*filter == ProjectFilter::All, |task| {
                filter.matches(task.project())
            }),
        (None, Some(chat)) if chat.kind == AssistantKind::Build => {
            *filter == ProjectFilter::All || filter.matches(source.project(feed))
        }
        // Chats belong to the workspace, not to a project.
        _ => true,
    }
}

fn sources<'a>(feed: &'a daemon::Feed) -> impl Iterator<Item = Source<'a>> {
    feed.agents
        .iter()
        .map(|(id, row)| Source {
            id: id.as_str(),
            run: Some(row),
            chat: None,
        })
        .chain(feed.assistants.iter().map(|(id, row)| Source {
            id: id.as_str(),
            run: None,
            chat: Some(row),
        }))
}

/// The sessions the project scope and Show filter keep, in roster order.
pub(super) fn scoped<'a>(
    feed: &'a daemon::Feed,
    filter: &ProjectFilter,
    show: Show,
) -> Vec<Source<'a>> {
    let filter = filter.resolved(feed);
    let mut rows: Vec<_> = sources(feed)
        .filter(|source| in_scope(*source, feed, &filter))
        .filter(|source| show == Show::All || source.live() || source.pinned(feed))
        .collect();
    rows.sort_by(|a, b| {
        a.state()
            .1
            .cmp(&b.state().1)
            .then_with(|| b.pinned(feed).cmp(&a.pinned(feed)))
            .then_with(|| b.activity().cmp(&a.activity()))
            .then_with(|| a.id.cmp(b.id))
    });
    rows
}

pub(super) fn visible(source: Source<'_>, role: &RosterRole) -> bool {
    match role {
        RosterRole::All => true,
        other => source.role() == other.slug(),
    }
}

pub(super) fn short(text: &str, max: usize) -> String {
    let mut result = String::with_capacity(text.len().min(max.saturating_mul(4)));
    let mut count = 0;
    for word in text.split_whitespace() {
        if !result.is_empty() {
            if count == max {
                result.push('…');
                return result;
            }
            result.push(' ');
            count += 1;
        }
        for ch in word.chars() {
            if count == max {
                result.push('…');
                return result;
            }
            result.push(ch);
            count += 1;
        }
    }
    result
}

/// The id a row shows: `T-12 R2` for a run, the chat's or build's own id.
fn shown_id(source: Source<'_>) -> String {
    match source.run {
        Some(run) => format!("{} {}", run.run.task, run.run.id),
        None => source.id.to_owned(),
    }
}

fn project_words(source: Source<'_>, feed: &daemon::Feed) -> String {
    match (source.run, source.chat) {
        (Some(_), _) if source.task(feed).is_none() => "project unknown".into(),
        (Some(_), _) => source.project(feed).unwrap_or("Unassigned").into(),
        (None, Some(chat)) => match &chat.build {
            Some(build) => {
                let project = build.project.as_deref().unwrap_or("workspace");
                match &build.branch {
                    Some(branch) => format!("{project} · {branch}"),
                    None => project.into(),
                }
            }
            None => "workspace".into(),
        },
        (None, None) => "–".into(),
    }
}

pub(super) fn row(source: Source<'_>, feed: &daemon::Feed) -> RosterRow {
    let (row_state, _) = source.state();
    let doing = source.doing(feed);
    let quiet = source.run.and_then(|row| live::quiet(row, feed));
    let agent = source.agent();
    let name = source.name(feed);
    let mut hints = vec![format!(
        "{} · {}",
        row_state.word,
        live::role_label(source.role())
    )];
    hints.extend(doing.clone());
    hints.extend(agent.clone());
    hints.extend(quiet.clone());
    let since = match (source.run, source.chat) {
        (Some(run), _) => live::clock_at(run.run.started, feed),
        (None, Some(chat)) => live::clock(chat.created.as_deref(), feed),
        (None, None) => None,
    };
    RosterRow {
        id: format!("agent-{}", source.id),
        session: Id::from(source.id),
        task: source.run.map(|row| Id::new(&row.run.task)),
        role: live::role_label(source.role()),
        shown_id: shown_id(source),
        named: name.is_some(),
        name: name.unwrap_or_else(|| "untitled".into()),
        project: project_words(source, feed),
        doing: doing
            .as_deref()
            .map_or_else(|| "–".into(), |text| short(text, 76)),
        doing_hint: hints.join(" · "),
        needs: source.needs(),
        agent: agent.unwrap_or_else(|| "–".into()),
        cost: source
            .chat
            .and_then(|chat| chat.cost_usd)
            .map_or_else(|| "–".into(), |cost| cost_text(Some(cost), false)),
        ctx: "–".into(),
        quiet: quiet.unwrap_or_else(|| "–".into()),
        since: since.unwrap_or_else(|| "–".into()),
        pinned: source.pinned(feed),
        resumable: source.resumable(),
        remove: source
            .removable()
            .then(|| Action::Remove(Id::from(source.id))),
        state: row_state,
        action: Action::OpenSession(Id::from(source.id)),
    }
}

fn one(n: usize, single: &'static str, many: &'static str) -> &'static str {
    if n == 1 { single } else { many }
}

/// Counts describe the roster before its role tab: working, waiting on the owner, idle, stale.
pub fn roster(state: &State, feed: &daemon::Feed, filter: &ProjectFilter) -> Roster {
    let sources = scoped(feed, filter, state.show);
    let (mut working, mut needs, mut idle, mut stale_count) = (0, 0, 0, 0);
    for source in &sources {
        match source.state().1 {
            NEEDS => needs += 1,
            WORKING => working += 1,
            IDLE => idle += 1,
            _ => {}
        }
        stale_count += usize::from(source.is_stale(feed));
    }
    let mut stats = Vec::new();
    for (key, icon, count, hue, hint) in [
        (
            "working",
            StatIcon::Moving,
            working,
            Hue::Worker,
            format!(
                "{working} {} working",
                one(working, "session is", "sessions are")
            ),
        ),
        (
            "needs-you",
            StatIcon::NeedsYou,
            needs,
            Hue::You,
            format!(
                "{needs} {} on you",
                one(needs, "session waits", "sessions wait")
            ),
        ),
        (
            "idle",
            StatIcon::Sessions,
            idle,
            Hue::Neutral,
            format!("{idle} {} idle", one(idle, "session is", "sessions are")),
        ),
        (
            "stale",
            StatIcon::Stale,
            stale_count,
            Hue::Held,
            format!(
                "{stale_count} {} last-known facts",
                one(stale_count, "session shows", "sessions show")
            ),
        ),
    ] {
        if count > 0 {
            stats.push(Stat::new(key, icon, count.to_string(), hue, hint));
        }
    }
    let mut roles = vec![
        RosterRole::All,
        RosterRole::Build,
        RosterRole::Assistant,
        RosterRole::Worker,
        RosterRole::Reviewer,
    ];
    for source in &sources {
        let role = RosterRole::of(source.role());
        if !roles.contains(&role) {
            roles.push(role);
        }
    }
    let roles = roles
        .into_iter()
        .filter_map(|role| {
            let count = sources
                .iter()
                .filter(|source| visible(**source, &role))
                .count();
            (count > 0 || role == RosterRole::All).then(|| RoleTab {
                id: format!("seg-agents-{}", role.slug()),
                label: role.label(),
                count,
                selected: role == state.roster_role,
                action: Action::SetRosterRole(role),
            })
        })
        .collect();
    let show = [Show::Live, Show::All]
        .into_iter()
        .map(|show| ShowTab {
            id: format!("seg-show-{}", show.slug()),
            label: show.label(),
            selected: show == state.show,
            action: Action::SetShow(show),
        })
        .collect();
    let rows: Vec<_> = sources
        .iter()
        .copied()
        .filter(|source| visible(*source, &state.roster_role))
        .map(|source| row(source, feed))
        .collect();
    let fill = if !rows.is_empty() {
        Fill::Ready
    } else if feed.agents.is_empty() && feed.assistants.is_empty() {
        let topics = [feed.topics.get("agents"), feed.topics.get("assistant")];
        if let Some(error) = topics
            .iter()
            .flatten()
            .find_map(|topic| topic.error.as_ref())
        {
            Fill::Failed(format!(
                "Sessions unavailable: {} ({})",
                error.message, error.code
            ))
        } else if topics.iter().all(Option::is_none)
            || topics.iter().flatten().any(|topic| topic.loading)
        {
            Fill::Loading("Reading sessions…")
        } else {
            Fill::Empty {
                heading: "No sessions yet.",
                paragraph: "Start a build or an assistant session with New; runs appear here when a task is dispatched or reviewed.".into(),
            }
        }
    } else {
        Fill::Empty {
            heading: "No sessions in this scope",
            paragraph:
                "Choose another project or role, or Show All for sessions that are not running."
                    .into(),
        }
    };
    Roster {
        title: "Agents",
        stats,
        roles,
        show,
        new: new_menu(),
        rows,
        columns: [
            "State",
            "Role",
            "ID",
            "Name",
            "Project",
            "Doing now",
            "Model",
            "$",
            "Ctx",
            "Quiet",
            "Started",
        ],
        fill,
        stale: (live::stale(feed, "agents") || live::stale(feed, "assistant")).then(|| {
            format!(
                "! Showing sessions from {} · corrald unreachable",
                feed.synced.as_deref().unwrap_or("an unknown time")
            )
        }),
    }
}
