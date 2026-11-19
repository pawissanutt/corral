//! One session per line: corrald's live runs, then the pinned quick assistant thread. Assistant
//! rows supplement run rows, never duplicate them.
use corral_protocol::rows::{AgentRow, AssistantRow};
use corral_types::SessionStatus;

use crate::model::board::Mark;
use crate::model::daemon;
use crate::model::effect::{Effect, Env};
use crate::model::glance::{Stat, StatIcon};
use crate::model::projects::ProjectFilter;
use crate::model::work::{Record, session_task};
use crate::model::{Hue, Id, Live, Selection, assistant};

use super::{Action, AgentStatus, State, live};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RosterRole {
    #[default]
    All,
    Worker,
    Reviewer,
    Assistant,
    Other(String),
}
impl RosterRole {
    pub fn of(role: &str) -> Self {
        match role {
            "worker" => Self::Worker,
            "reviewer" => Self::Reviewer,
            "assistant" => Self::Assistant,
            other => Self::Other(other.into()),
        }
    }
    pub fn slug(&self) -> &str {
        match self {
            Self::All => "all",
            Self::Worker => "worker",
            Self::Reviewer => "reviewer",
            Self::Assistant => "assistant",
            Self::Other(role) => role,
        }
    }
    fn label(&self) -> String {
        match self {
            Self::All => "All".into(),
            Self::Worker => "Workers".into(),
            Self::Reviewer => "Reviewers".into(),
            Self::Assistant => "Assistant".into(),
            Self::Other(role) => live::role_label(role),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterRow {
    pub id: String,
    pub session: Id,
    /// The run's task, whose Actions menu the row carries; none for an assistant.
    pub task: Option<Id>,
    pub chip: String,
    pub hue: Hue,
    pub role: String,
    pub shown_id: String,
    pub project: String,
    pub name: String,
    pub pinned: bool,
    pub doing: String,
    pub doing_hint: String,
    /// The run's profile and model, as recorded when it started.
    pub agent: String,
    /// How long the session has written nothing (`quiet 12m`), or a dash.
    pub quiet: String,
    pub since: String,
    pub selected: bool,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roster {
    pub title: &'static str,
    pub stats: Vec<Stat>,
    pub roles: Vec<RoleTab>,
    pub rows: Vec<RosterRow>,
    pub columns: [&'static str; 9],
    pub fill: super::Fill,
    pub stale: Option<String>,
}

/// A roster line's source: a live run (keyed `<task>_<run>`), an assistant session, or both.
#[derive(Clone, Copy)]
pub(super) struct Source<'a> {
    pub id: &'a str,
    pub row: Option<&'a AgentRow>,
    pub assistant: Option<&'a AssistantRow>,
}
impl<'a> Source<'a> {
    pub fn role(self) -> &'a str {
        self.row.map_or("assistant", |row| row.run.role.as_str())
    }
    pub fn workspace(self) -> bool {
        self.row.is_none()
    }
    fn task(self, feed: &'a daemon::Feed) -> Option<&'a daemon::Task> {
        self.row.and_then(|row| feed.board.task(&row.run.task))
    }
    pub fn project(self, feed: &'a daemon::Feed) -> Option<&'a str> {
        self.task(feed).and_then(|task| task.project())
    }
    pub fn pinned(self, feed: &daemon::Feed) -> bool {
        self.row.is_none() && assistant::quick_id(feed).as_deref() == Some(self.id)
    }
    fn status(self) -> Option<AgentStatus> {
        self.row.map(|row| live::status(&row.status))
    }
    pub fn chip(self, feed: &daemon::Feed) -> (String, Hue) {
        match self.status() {
            Some(status) => {
                let (label, hue) = status.chip();
                (label.into(), hue)
            }
            None => {
                let chip = assistant::live_chip(feed, self.id);
                (chip.label, chip.hue)
            }
        }
    }
    pub fn name(self, feed: &daemon::Feed) -> String {
        if let Some(row) = self.assistant {
            return row.title.clone();
        }
        self.task(feed)
            .map(|task| task.title().to_owned())
            .unwrap_or_default()
    }
    /// What the session is doing: a blocked screen's prompt, the task's progress while the
    /// session works, else what an idle or ended session says of itself. A turn that was
    /// cancelled or a session that was terminated leaves the task Running, so the task's line
    /// would still read "running".
    pub fn doing(self, feed: &daemon::Feed) -> String {
        if let Some(row) = self.row {
            let task = self.task(feed);
            let said = match &row.status {
                SessionStatus::Blocked { .. } => live::excerpt(row)
                    .map(str::to_owned)
                    .or_else(|| task.map(daemon::Task::now)),
                SessionStatus::Working => task.map(daemon::Task::now),
                SessionStatus::Idle if row.paused == Some(true) => {
                    Some("paused · its input waits for Resume".into())
                }
                SessionStatus::Idle => Some(
                    task.and_then(daemon::Task::blocked)
                        .map_or_else(|| "idle · no turn running".into(), |b| b.reason.clone()),
                ),
                SessionStatus::Gone => Some("session ended".into()),
            };
            return said
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| "Doing now unknown".into());
        }
        let scope = if self.pinned(feed) {
            assistant::QUICK
        } else {
            self.id
        };
        feed.threads
            .get(scope)
            .and_then(|thread| {
                thread
                    .entries
                    .values()
                    .filter(|entry| entry.kind == corral_protocol::assistant::EntryKind::Answer)
                    .max_by_key(|entry| live::unix(&entry.at))
                    .and_then(|entry| entry.text.clone())
            })
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| "Doing now unknown".into())
    }
    /// The run's profile and model; an assistant's profile.
    pub fn agent(self) -> Option<String> {
        if let Some(row) = self.row {
            let model = row.run.model.as_deref().unwrap_or("model unknown");
            return Some(format!("{} · {model}", row.run.profile));
        }
        self.assistant.and_then(|row| row.profile.clone())
    }
    pub fn is_stale(self, feed: &daemon::Feed) -> bool {
        live::stale(
            feed,
            if self.row.is_some() {
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
    match source.row {
        None => true,
        Some(row) => feed
            .board
            .task(&row.run.task)
            .map_or(*filter == ProjectFilter::All, |task| {
                filter.matches(task.project())
            }),
    }
}

pub(super) fn selected<'a>(
    feed: &'a daemon::Feed,
    id: &str,
    filter: &ProjectFilter,
) -> Option<Source<'a>> {
    let (key, row) = feed
        .agents
        .get_key_value(id)
        .map_or((None, None), |(key, row)| (Some(key), Some(row)));
    let assistant = feed.assistants.get_key_value(id);
    let source = Source {
        id: key.or(assistant.map(|(key, _)| key))?.as_str(),
        row,
        assistant: assistant.map(|(_, row)| row),
    };
    (in_scope(source, feed, &filter.resolved(feed))
        && (source.row.is_some() || source.pinned(feed)))
    .then_some(source)
}

pub(super) fn scoped<'a>(feed: &'a daemon::Feed, filter: &ProjectFilter) -> Vec<Source<'a>> {
    let filter = filter.resolved(feed);
    let quick = assistant::quick_id(feed);
    let mut rows: Vec<_> = feed
        .agents
        .iter()
        .map(|(id, row)| Source {
            id: id.as_str(),
            row: Some(row),
            assistant: None,
        })
        .chain(
            feed.assistants
                .iter()
                .filter(|(id, _)| quick.as_deref() == Some(id.as_str()))
                .map(|(id, row)| Source {
                    id: id.as_str(),
                    row: None,
                    assistant: Some(row),
                }),
        )
        .filter(|source| in_scope(*source, feed, &filter))
        .collect();
    rows.sort_by(|a, b| {
        let rank = |source: Source<'_>| {
            if source.row.is_none() {
                0
            } else if source.status() == Some(AgentStatus::Blocked) {
                1
            } else if source.is_stale(feed) {
                2
            } else {
                3
            }
        };
        rank(*a).cmp(&rank(*b)).then_with(|| a.id.cmp(b.id))
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

pub(super) fn row(source: Source<'_>, state: &State, feed: &daemon::Feed) -> RosterRow {
    let (chip, hue) = source.chip(feed);
    let full = source.doing(feed);
    let quiet = source.row.and_then(|row| live::quiet(row, feed));
    let agent = source.agent();
    let mut hints = vec![
        format!("{chip} · {}", live::role_label(source.role())),
        full.clone(),
    ];
    hints.extend(agent.clone());
    hints.extend(quiet.clone());
    RosterRow {
        id: format!("agent-{}", source.id),
        session: Id::from(source.id),
        task: source.row.map(|row| Id::new(&row.run.task)),
        chip,
        hue,
        role: live::role_label(source.role()),
        shown_id: source
            .row
            .map(live::shown_id)
            .unwrap_or_else(|| source.id.into()),
        project: if source.workspace() {
            "workspace".into()
        } else if source.task(feed).is_none() {
            "project unknown".into()
        } else {
            source.project(feed).unwrap_or("Unassigned").into()
        },
        name: source.name(feed),
        pinned: source.pinned(feed),
        doing: short(&full, 76),
        doing_hint: hints.join(" · "),
        agent: agent.unwrap_or_else(|| "—".into()),
        quiet: quiet.unwrap_or_else(|| "—".into()),
        since: source
            .row
            .and_then(|row| live::clock_at(row.run.started, feed))
            .unwrap_or_else(|| "—".into()),
        selected: state
            .roster_session
            .as_ref()
            .is_some_and(|id| id.as_str() == source.id),
        action: Action::SelectRoster(Id::from(source.id)),
    }
}

/// Counts describe the roster before its role tab: live runs and the pinned quick thread.
/// Workspace sessions do not inflate a project's run count.
pub fn roster(state: &State, feed: &daemon::Feed, _env: Env, filter: &ProjectFilter) -> Roster {
    let sources = scoped(feed, filter);
    let scoped_project = filter.resolved(feed) != ProjectFilter::All;
    let (mut sessions, mut needs, mut stale_count) = (0, 0, 0);
    for source in sources
        .iter()
        .filter(|source| !scoped_project || !source.workspace())
    {
        sessions += 1;
        needs += usize::from(source.status() == Some(AgentStatus::Blocked));
        stale_count += usize::from(source.is_stale(feed));
    }
    let mut stats = Vec::new();
    let one =
        |n: usize, single: &'static str, many: &'static str| if n == 1 { single } else { many };
    for (key, icon, count, hue, hint) in [
        (
            "sessions",
            StatIcon::Sessions,
            sessions,
            Hue::Neutral,
            if scoped_project {
                format!(
                    "{sessions} {} in this project scope. The pinned quick thread stays listed.",
                    one(sessions, "live run works", "live runs work")
                )
            } else {
                format!(
                    "{sessions} {}: live runs and the pinned quick thread.",
                    one(sessions, "session", "sessions")
                )
            },
        ),
        (
            "needs-you",
            StatIcon::NeedsYou,
            needs,
            Hue::You,
            format!(
                "{needs} {} blocked at a prompt",
                one(needs, "session is", "sessions are")
            ),
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
        RosterRole::Worker,
        RosterRole::Reviewer,
        RosterRole::Assistant,
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
    let rows: Vec<_> = sources
        .iter()
        .copied()
        .filter(|source| visible(*source, &state.roster_role))
        .map(|source| row(source, state, feed))
        .collect();
    let fill = if !rows.is_empty() {
        super::Fill::Ready
    } else if sources.is_empty() && feed.agents.is_empty() && feed.assistants.is_empty() {
        let topics = [feed.topics.get("agents"), feed.topics.get("assistant")];
        if let Some(error) = topics
            .iter()
            .flatten()
            .find_map(|topic| topic.error.as_ref())
        {
            super::Fill::Failed(format!(
                "Sessions unavailable: {} ({})",
                error.message, error.code
            ))
        } else if topics.iter().all(Option::is_none)
            || topics.iter().flatten().any(|topic| topic.loading)
        {
            super::Fill::Loading("Loading sessions…")
        } else {
            super::Fill::Empty {
                heading: "No live runs",
                paragraph: "A run appears here when a task is dispatched or reviewed.".into(),
            }
        }
    } else {
        super::Fill::Empty {
            heading: "No sessions in this scope or role",
            paragraph: "Choose another project or role to see its sessions.".into(),
        }
    };
    Roster {
        title: "Agents",
        stats,
        roles,
        rows,
        columns: [
            "State",
            "ID",
            "Role",
            "Project",
            "Session",
            "Doing now",
            "Agent",
            "Quiet",
            "Started",
        ],
        fill,
        stale: (live::stale(feed, "agents") || live::stale(feed, "assistant")).then(|| {
            format!(
                "Last known sessions · {}",
                feed.synced.as_deref().unwrap_or("time unknown")
            )
        }),
    }
}

impl State {
    /// The roster's keys: j/k (and the arrows) move the selection, Escape clears it, and `.`
    /// opens the Session menu of the selected run, or of the run whose page is open.
    pub fn key_live_filtered(
        &mut self,
        key: &str,
        top: Option<&Live>,
        _selection: &Selection,
        feed: &daemon::Feed,
        _env: Env,
        filter: &ProjectFilter,
    ) -> Option<Vec<Effect>> {
        if key == "." {
            let session = match top {
                Some(Live::Session(id)) => Some(id.clone()),
                Some(_) => return None,
                None => self.roster_session.clone(),
            };
            return Some(
                session
                    .filter(|id| session_task(id).is_some())
                    .map(|id| vec![Effect::VerbMenu(Record::Session(id))])
                    .unwrap_or_default(),
            );
        }
        if top.is_some() {
            return None;
        }
        if key == "escape" && self.roster_session.take().is_some() {
            return Some(vec![]);
        }
        let direction = match key {
            "j" | "down" => 1,
            "k" | "up" => -1,
            _ => return None,
        };
        let visible: Vec<_> = scoped(feed, filter)
            .into_iter()
            .filter(|source| visible(*source, &self.roster_role))
            .collect();
        if visible.is_empty() {
            return Some(vec![]);
        }
        let current = self
            .roster_session
            .as_ref()
            .and_then(|id| visible.iter().position(|source| source.id == id.as_str()));
        let at = match (current, direction) {
            (Some(at), 1) => (at + 1).min(visible.len() - 1),
            (Some(at), _) => at.saturating_sub(1),
            (None, _) => 0,
        };
        self.roster_session = Some(Id::from(visible[at].id));
        self.roster_tab = super::RosterPanelTab::Summary;
        Some(vec![])
    }
}

impl State {
    /// Reply and panel presses target the roster's session without changing the route.
    pub fn act_roster(&mut self, action: Action, feed: &daemon::Feed, env: Env) -> Vec<Effect> {
        let selection = Selection {
            session: self.roster_session.clone(),
            ..Selection::default()
        };
        self.act_live(action, feed, &selection, env)
    }
}
