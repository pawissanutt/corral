//! One session per line. Assistant rows supplement agent rows, never duplicate them.
use corral_protocol::rows::{AgentRow, AgentStatus, AssistantRow};

use crate::model::board::Mark;
use crate::model::daemon;
use crate::model::effect::{Effect, Env};
use crate::model::glance::{Activity, Stat, StatIcon};
use crate::model::projects::ProjectFilter;
use crate::model::{Hue, Id, Live, Selection, assistant};

use super::{Action, State, live};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RosterRole {
    #[default]
    All,
    Worker,
    Gate,
    Reviewer,
    Planner,
    Pm,
    Assistant,
    Other(String),
}
impl RosterRole {
    pub fn of(role: &str) -> Self {
        match role {
            "worker" => Self::Worker,
            "gate" => Self::Gate,
            "reviewer" | "plan_reviewer" => Self::Reviewer,
            "planner" => Self::Planner,
            role if role.eq_ignore_ascii_case("pm") => Self::Pm,
            "assistant" => Self::Assistant,
            other => Self::Other(other.into()),
        }
    }
    pub fn slug(&self) -> &str {
        match self {
            Self::All => "all",
            Self::Worker => "worker",
            Self::Gate => "gate",
            Self::Reviewer => "reviewer",
            Self::Planner => "planner",
            Self::Pm => "pm",
            Self::Assistant => "assistant",
            Self::Other(role) => role,
        }
    }
    fn label(&self) -> String {
        match self {
            Self::All => "All".into(),
            Self::Worker => "Workers".into(),
            Self::Gate => "Gates".into(),
            Self::Reviewer => "Reviewers".into(),
            Self::Planner => "Planners".into(),
            Self::Pm => live::role_label("pm"),
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
    pub chip: String,
    pub hue: Hue,
    pub role: String,
    pub shown_id: String,
    pub project: String,
    pub name: String,
    pub pinned: bool,
    pub doing: String,
    pub doing_hint: String,
    pub activity: Option<Activity>,
    pub last_output: String,
    pub since: String,
    pub cost: String,
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
    pub unknown_activity: &'static str,
    /// Launch PM, while no PM is live.
    pub launch: Option<super::Launch>,
}

#[derive(Clone, Copy)]
pub(super) struct Source<'a> {
    pub row: Option<&'a AgentRow>,
    pub assistant: Option<&'a AssistantRow>,
}
impl<'a> Source<'a> {
    pub fn id(self) -> &'a str {
        self.row
            .map(|row| row.id.as_str())
            .or_else(|| self.assistant.map(|row| row.id.as_str()))
            .unwrap_or("")
    }
    pub fn role(self) -> &'a str {
        self.row.map_or("assistant", |row| row.role.as_str())
    }
    pub fn workspace(self) -> bool {
        self.role().eq_ignore_ascii_case("pm") || self.role() == "assistant"
    }
    pub fn project(self, feed: &'a daemon::Feed) -> Option<&'a str> {
        self.row
            .and_then(|row| row.task.as_deref())
            .and_then(|id| feed.board.task(id))
            .and_then(|task| task.project.as_deref())
    }
    pub fn pinned(self, feed: &daemon::Feed) -> bool {
        self.role() == "assistant" && assistant::quick_id(feed).as_deref() == Some(self.id())
    }
    pub fn chip(self, feed: &daemon::Feed) -> (String, Hue) {
        if let Some(row) = self.row {
            let (label, hue) = live::status(row.status).chip();
            (label.into(), hue)
        } else {
            let chip = assistant::live_chip(feed, self.id());
            (chip.label, chip.hue)
        }
    }
    pub fn name(self) -> String {
        self.assistant
            .map(|row| row.title.clone())
            .or_else(|| self.row.map(|row| row.title.clone()))
            .unwrap_or_default()
    }
    pub fn doing(self, feed: &daemon::Feed) -> String {
        if let Some(row) = self.row {
            return if row.now.trim().is_empty() {
                "Doing now unknown".into()
            } else {
                row.now.clone()
            };
        }
        let scope = if self.pinned(feed) {
            assistant::QUICK
        } else {
            self.id()
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
    pub fn is_stale(self, feed: &daemon::Feed) -> bool {
        live::stale(
            feed,
            if self.row.is_some() {
                "agents"
            } else {
                "assistant"
            },
        ) || self
            .row
            .and_then(|row| row.task.as_deref())
            .and_then(|id| feed.board.task(id))
            .is_some_and(|task| task.rail.iter().any(|step| step.mark == Mark::Stale))
    }
}

fn in_scope(source: Source<'_>, feed: &daemon::Feed, filter: &ProjectFilter) -> bool {
    source.workspace()
        || match source.row.and_then(|row| row.task.as_deref()) {
            Some(id) => feed
                .board
                .task(id)
                .map_or(*filter == ProjectFilter::All, |task| {
                    filter.matches(task.project.as_deref())
                }),
            None => filter.matches(None),
        }
}

pub(super) fn selected<'a>(
    feed: &'a daemon::Feed,
    id: &str,
    filter: &ProjectFilter,
) -> Option<Source<'a>> {
    let source = Source {
        row: feed.agents.get(id),
        assistant: feed.assistants.get(id),
    };
    (source.row.is_some() || source.assistant.is_some())
        .then_some(source)
        .filter(|source| {
            in_scope(*source, feed, &filter.resolved(feed))
                && (source.role() != "assistant" || source.pinned(feed))
        })
}

pub(super) fn scoped<'a>(feed: &'a daemon::Feed, filter: &ProjectFilter) -> Vec<Source<'a>> {
    let filter = filter.resolved(feed);
    let quick = assistant::quick_id(feed);
    let mut rows: Vec<_> = feed
        .agents
        .values()
        .map(|row| Source {
            row: Some(row),
            assistant: feed.assistants.get(row.id.as_str()),
        })
        .chain(
            feed.assistants
                .values()
                .filter(|row| !feed.agents.contains_key(row.id.as_str()))
                .map(|row| Source {
                    row: None,
                    assistant: Some(row),
                }),
        )
        .filter(|source| source.role() != "assistant" || quick.as_deref() == Some(source.id()))
        .filter(|source| in_scope(*source, feed, &filter))
        .collect();
    rows.sort_by(|a, b| {
        let rank = |source: Source<'_>| {
            if quick.as_deref() == Some(source.id()) {
                0
            } else if source
                .row
                .is_some_and(|row| row.status == AgentStatus::Asking)
            {
                1
            } else if source.is_stale(feed) {
                2
            } else {
                3
            }
        };
        rank(*a).cmp(&rank(*b)).then_with(|| a.id().cmp(b.id()))
    });
    rows
}

pub(super) fn visible(source: Source<'_>, role: &RosterRole) -> bool {
    match role {
        RosterRole::All => true,
        RosterRole::Reviewer => matches!(source.role(), "reviewer" | "plan_reviewer"),
        RosterRole::Pm => source.role().eq_ignore_ascii_case("pm"),
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

pub(super) fn last_output(at: Option<&str>, feed: &daemon::Feed) -> String {
    let Some(at) = at
        .and_then(live::unix)
        .filter(|at| feed.now > 0 && *at <= feed.now)
    else {
        return "—".into();
    };
    let age = daemon::age(Some(at), feed.now);
    if feed.now - at >= 5 * 60 {
        format!("quiet {age}")
    } else {
        format!("{age} ago")
    }
}

fn current_activity(window: &corral_protocol::rows::AgentActivity, feed: &daemon::Feed) -> bool {
    live::unix(&window.until)
        .is_some_and(|until| feed.now > 0 && until <= feed.now && feed.now - until <= 2 * 60)
}

fn activity_time(at: &str, feed: &daemon::Feed) -> String {
    let label = (feed.now > 0)
        .then(|| crate::model::work::when(at, feed))
        .unwrap_or_default();
    if label.is_empty() {
        "time unknown".into()
    } else {
        label
    }
}

pub(super) fn activity_axis(row: Option<&AgentRow>, feed: &daemon::Feed) -> [String; 3] {
    let Some(window) = row
        .and_then(|row| row.activity.as_ref())
        .filter(|_| feed.capable(corral_protocol::AGENTS_ACTIVITY_V1))
    else {
        return ["—".into(), "events per minute".into(), "—".into()];
    };
    let minutes = window.per_minute.len();
    if current_activity(window, feed) {
        return [
            format!("{minutes} min ago"),
            "events per minute".into(),
            "now".into(),
        ];
    }
    let start = live::unix(&window.until)
        .and_then(|until| until.checked_sub(u64::try_from(minutes).ok()?.checked_mul(60)?))
        .and_then(|second| i64::try_from(second).ok())
        .and_then(|second| jiff::Timestamp::from_second(second).ok())
        .map(|at| activity_time(&at.to_string(), feed))
        .unwrap_or_else(|| "time unknown".into());
    [
        start,
        "events per minute".into(),
        activity_time(&window.until, feed),
    ]
}

pub(super) fn activity(row: Option<&AgentRow>, feed: &daemon::Feed, hue: Hue) -> Option<Activity> {
    if !feed.capable(corral_protocol::AGENTS_ACTIVITY_V1) {
        return None;
    }
    let window = row?.activity.as_ref()?;
    let counts = &window.per_minute;
    let total: u64 = counts.iter().map(|&n| u64::from(n)).sum();
    Some(Activity {
        per_minute: counts.clone(),
        hue,
        hint: if current_activity(window, feed) {
            format!("{total} events in the last {} min", counts.len())
        } else {
            format!(
                "{total} events in the {} min to {}",
                counts.len(),
                activity_time(&window.until, feed)
            )
        },
    })
}

pub(super) fn row(source: Source<'_>, state: &State, feed: &daemon::Feed) -> RosterRow {
    let (chip, hue) = source.chip(feed);
    let full = source.doing(feed);
    let last = last_output(
        source.row.and_then(|row| row.last_output_at.as_deref()),
        feed,
    );
    let mut hints = vec![
        format!("{chip} · {}", live::role_label(source.role())),
        full.clone(),
    ];
    if let Some(steps) = source
        .row
        .and_then(|row| row.steps.as_deref())
        .filter(|steps| !steps.is_empty())
    {
        if let Some((at, step)) = steps
            .iter()
            .enumerate()
            .find(|(_, step)| step.state == corral_protocol::rows::AgentStepState::InProgress)
        {
            hints.push(format!(
                "step {} of {}: {}",
                at + 1,
                steps.len(),
                step.title
            ));
        } else {
            let done = steps
                .iter()
                .filter(|step| step.state == corral_protocol::rows::AgentStepState::Done)
                .count();
            hints.push(format!("{done} of {} steps done", steps.len()));
        }
    }
    hints.push(if last == "—" {
        "Last output unknown".into()
    } else {
        format!("Last output: {last}")
    });
    let cost = source
        .row
        .and_then(|row| live::usage(row.cost_usd, row.cost_basis, row.tokens))
        .or_else(|| {
            source
                .assistant
                .and_then(|row| live::usage(row.cost_usd, row.cost_basis, None))
        });
    RosterRow {
        id: format!("agent-{}", source.id()),
        session: Id::from(source.id()),
        chip,
        hue,
        role: live::role_label(source.role()),
        shown_id: if source.role().eq_ignore_ascii_case("pm")
            && source.row.is_some_and(|row| row.task.is_none())
        {
            "PM".into()
        } else {
            source
                .row
                .map(live::shown_id)
                .unwrap_or_else(|| source.id().into())
        },
        project: if source.workspace() {
            "workspace".into()
        } else if source
            .row
            .and_then(|row| row.task.as_deref())
            .is_some_and(|id| feed.board.task(id).is_none())
        {
            "project unknown".into()
        } else {
            source.project(feed).unwrap_or("Unassigned").into()
        },
        name: source.name(),
        pinned: source.pinned(feed),
        doing: short(&full, 76),
        doing_hint: hints.join(" · "),
        activity: activity(source.row, feed, hue),
        last_output: last,
        since: source
            .row
            .and_then(|row| live::clock(row.since.as_deref(), feed))
            .unwrap_or_else(|| "—".into()),
        cost: cost.unwrap_or_else(|| "—".into()),
        selected: state
            .roster_session
            .as_ref()
            .is_some_and(|id| id.as_str() == source.id()),
        action: Action::SelectRoster(Id::from(source.id())),
    }
}

/// Counts describe the roster before its role tab: task sessions, the PM and the pinned quick
/// thread. Named assistant sessions have their own section; workspace sessions do not inflate
/// a project's task-session count.
pub fn roster(state: &State, feed: &daemon::Feed, env: Env, filter: &ProjectFilter) -> Roster {
    let sources = scoped(feed, filter);
    let scoped_project = filter.resolved(feed) != ProjectFilter::All;
    let (mut sessions, mut needs, mut stale_count) = (0, 0, 0);
    for source in sources
        .iter()
        .filter(|source| !scoped_project || !source.workspace())
    {
        sessions += 1;
        needs += usize::from(
            source
                .row
                .is_some_and(|row| row.status == AgentStatus::Asking),
        );
        stale_count += usize::from(source.is_stale(feed));
    }
    let mut stats = Vec::new();
    for (key, icon, count, hue, hint) in [
        (
            "sessions",
            StatIcon::Sessions,
            sessions,
            Hue::Neutral,
            if scoped_project {
                format!(
                    "{sessions} task sessions work in this project scope. The workspace PM and pinned quick thread stay listed; named assistant sessions are counted below."
                )
            } else {
                format!(
                    "{sessions} roster sessions: task sessions, the workspace PM and the pinned quick thread. Named assistant sessions are counted below."
                )
            },
        ),
        (
            "needs-you",
            StatIcon::NeedsYou,
            needs,
            Hue::You,
            format!("{needs} sessions need you"),
        ),
        (
            "stale",
            StatIcon::Stale,
            stale_count,
            Hue::Held,
            format!("{stale_count} sessions have stale task results or last-known session facts"),
        ),
    ] {
        if count > 0 {
            stats.push(Stat::new(key, icon, count.to_string(), hue, hint));
        }
    }
    let mut roles = vec![
        RosterRole::All,
        RosterRole::Worker,
        RosterRole::Gate,
        RosterRole::Reviewer,
        RosterRole::Planner,
        RosterRole::Pm,
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
                heading: "No sessions",
                paragraph: "Sessions appear here when they start.".into(),
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
            "Project",
            "Session",
            "Doing now",
            "Activity",
            "Last output",
            "Since",
            "Cost",
        ],
        fill,
        stale: (live::stale(feed, "agents") || live::stale(feed, "assistant")).then(|| {
            format!(
                "Last known sessions · {}",
                feed.synced.as_deref().unwrap_or("time unknown")
            )
        }),
        unknown_activity: "—",
        launch: super::controls::launch(state, feed, env),
    }
}

impl State {
    pub fn key_live_filtered(
        &mut self,
        key: &str,
        top: Option<&Live>,
        _selection: &Selection,
        feed: &daemon::Feed,
        _env: Env,
        filter: &ProjectFilter,
    ) -> Option<Vec<Effect>> {
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
        let sources = scoped(feed, filter);
        let visible: Vec<_> = sources
            .into_iter()
            .filter(|source| visible(*source, &self.roster_role))
            .collect();
        if visible.is_empty() {
            return Some(vec![]);
        }
        let current = self
            .roster_session
            .as_ref()
            .and_then(|id| visible.iter().position(|source| source.id() == id.as_str()));
        let at = match (current, direction) {
            (Some(at), 1) => (at + 1).min(visible.len() - 1),
            (Some(at), _) => at.saturating_sub(1),
            (None, _) => 0,
        };
        self.roster_session = Some(Id::from(visible[at].id()));
        self.roster_tab = super::RosterPanelTab::Summary;
        Some(vec![])
    }
}

impl State {
    /// Reply and pane controls target the roster's session without changing the route.
    pub fn act_roster(&mut self, action: Action, feed: &daemon::Feed, env: Env) -> Vec<Effect> {
        let selection = Selection {
            session: self.roster_session.clone(),
            ..Selection::default()
        };
        self.act_live(action, feed, &selection, env)
    }
}
