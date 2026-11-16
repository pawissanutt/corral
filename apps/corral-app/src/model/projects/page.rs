//! Local project pages reuse served summaries; opening one requires no new daemon RPC. The page
//! is Now scoped to one project ([`project_now`]) beside the project's own facts, with its
//! plans, its sessions, its builds and its burn-up.
use std::borrow::Cow;

use super::{NowGroup, ProjectFilter, contains_project, is_open};
use crate::model::agents::{session_shown_id, session_status};
use crate::model::work::{self, NowPage, NowRow, Press};
use crate::model::{Form, Hue, Id, Live, Selection, daemon::Feed, timeline::BurnupScope};
use corral_protocol::detail::DetailRef;
use corral_protocol::rows::{AgentStatus, PlanState};

pub const PLANS_HEADING: &str = "Plans";
pub const PLANS_HINT: &str = "Plans that name this project. A row opens the plan.";
pub const SESSIONS_HEADING: &str = "Sessions";
pub const SESSIONS_HINT: &str =
    "Agent sessions working on this project's tasks. A row opens the session.";
pub const BUILDS_HEADING: &str = "Recent builds";
pub const NO_BUILDS: &str = "No published builds for this project.";
/// The side column's name for screen readers.
pub const FACTS_LABEL: &str = "Project facts";
/// What a count corrald does not report shows: unknown, never zero.
const UNKNOWN_COUNT: &str = "—";
const UNKNOWN: &str = "unknown";
const NO_STATISTICS: &str = "corrald does not report project statistics.";

/// One line of the side column's facts: its label, its value and the sentence behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFact {
    pub label: &'static str,
    pub value: String,
    pub hint: String,
    /// Paths and counts read in mono.
    pub mono: bool,
    /// `project-count-<slug>` on the counts.
    pub test_id: Option<String>,
}
/// A plan naming the project: its id, title and state chip; a press opens it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPlan {
    pub id: Id,
    pub title: String,
    pub state: &'static str,
    pub hue: Hue,
    pub open: Press,
}
/// A session whose task belongs to the project: its id, name and state chip; what it is doing
/// in its hint; a press opens it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSession {
    pub id: Id,
    /// The id the row shows, as the Agents roster shows it: the session's task, else itself.
    pub shown_id: String,
    pub name: String,
    pub state: &'static str,
    pub hue: Hue,
    pub doing: String,
    pub open: Press,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectBuild {
    pub id: Id,
    pub title: String,
    pub state: String,
}
/// Said in place of the owner moves and moving tasks while the project has no open work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectEmpty {
    pub heading: &'static str,
    pub body: &'static str,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPage {
    pub id: String,
    pub project: Id,
    pub title: String,
    pub facts: Vec<ProjectFact>,
    pub plans: Vec<ProjectPlan>,
    pub sessions: Vec<ProjectSession>,
    pub builds: Vec<ProjectBuild>,
    pub burnup_request: DetailRef,
    pub empty: Option<ProjectEmpty>,
    pub open_now_label: &'static str,
    pub ask_label: &'static str,
    pub ask_hint: &'static str,
}

/// A project page whose project corrald no longer lists: the heading and why.
pub fn not_found(id: &Id) -> (&'static str, String) {
    (
        "Project not found",
        format!(
            "corrald does not list a project named “{id}”. It may have been removed from the workspace configuration."
        ),
    )
}

/// Now's page in the project's scope, grouped by who acts next, with nothing selected: the
/// page has no side panel of its own, so a row opens its record (a task on Now, a question,
/// plan or review on its page) and the side column always holds the project's digest.
pub fn project_now(state: &work::State, feed: &Feed, form: Form, project: &Id) -> NowPage {
    // A selection made on Now must not take this page's side column from the digest.
    let state = if state.selected_ask.is_none() && state.digest_only {
        Cow::Borrowed(state)
    } else {
        Cow::Owned(work::State {
            selected_ask: None,
            digest_only: true,
            ..state.clone()
        })
    };
    let mut page = work::now_page(
        &state,
        feed,
        &Selection::default(),
        form,
        &ProjectFilter::Project(project.clone()),
        NowGroup::State,
    );
    for row in page
        .sections
        .iter_mut()
        .flat_map(|section| section.rows.iter_mut())
    {
        match row {
            NowRow::Ask(ask) => {
                if let Some(open) = ask.button.action.clone() {
                    ask.select = open;
                }
            }
            NowRow::Task(task) => opens(&mut task.action),
        }
    }
    for task in page
        .waiting
        .groups
        .iter_mut()
        .flat_map(|group| group.rows.iter_mut())
    {
        opens(&mut task.action);
    }
    page
}

/// A row's in-place selection becomes opening the task.
fn opens(action: &mut Press) {
    if let Press::Select(id) = action {
        *action = Press::Open(Live::Task(id.clone()));
    }
}

/// The plan state's chip: its word and hue.
fn plan_chip(state: PlanState) -> (&'static str, Hue) {
    match state {
        PlanState::Drafting => ("drafting", Hue::Neutral),
        PlanState::Reviewing => ("in plan review", Hue::Corral),
        PlanState::PlanFailed => ("plan failed", Hue::Fail),
        PlanState::Awaiting => ("needs you", Hue::You),
        PlanState::Executing => ("executing", Hue::Corral),
        PlanState::Approved => ("approved", Hue::Ok),
        PlanState::Complete => ("complete", Hue::Ok),
        PlanState::Cancelled => ("cancelled", Hue::Neutral),
        PlanState::Other => ("state unknown", Hue::Neutral),
    }
}

/// The project's agent sessions: those whose task is on the board under this project, as the
/// Agents roster joins them. Workspace sessions (the PM) serve every project and are not listed.
fn sessions(feed: &Feed, id: &Id) -> Vec<ProjectSession> {
    let mut rows: Vec<_> = feed
        .agents
        .values()
        .filter(|row| !row.role.eq_ignore_ascii_case("pm"))
        .filter(|row| {
            row.task
                .as_deref()
                .and_then(|task| feed.board.task(task))
                .is_some_and(|task| task.project.as_deref() == Some(id.as_str()))
        })
        .collect();
    rows.sort_by(|a, b| {
        (a.status != AgentStatus::Asking, &a.id).cmp(&(b.status != AgentStatus::Asking, &b.id))
    });
    rows.into_iter()
        .map(|row| {
            let (state, hue) = session_status(row.status).chip();
            let id = Id::from(row.id.as_str());
            ProjectSession {
                open: Press::Open(Live::Session(id.clone())),
                id,
                shown_id: session_shown_id(row),
                name: row.title.clone(),
                state,
                hue,
                doing: if row.now.trim().is_empty() {
                    "Doing now unknown".into()
                } else {
                    row.now.clone()
                },
            }
        })
        .collect()
}

/// Path, PM scope, autopilot, open and running, each unknown while corrald does not report it.
fn facts(
    path: Option<&str>,
    pm: Option<bool>,
    autopilot: Option<bool>,
    open: Option<u32>,
    running: Option<u32>,
) -> Vec<ProjectFact> {
    let word = |known: Option<bool>, on: &str, off: &str| {
        known
            .map_or(UNKNOWN, |known| if known { on } else { off })
            .to_owned()
    };
    let count = |slug: &str, label: &'static str, value: Option<u32>, hint: &str| ProjectFact {
        label,
        value: value.map_or_else(|| UNKNOWN_COUNT.to_owned(), |n| n.to_string()),
        hint: if value.is_some() {
            hint.into()
        } else {
            NO_STATISTICS.into()
        },
        mono: true,
        test_id: Some(format!("project-count-{slug}")),
    };
    vec![
        ProjectFact {
            label: "Path",
            value: path.unwrap_or(UNKNOWN).to_owned(),
            // The value may be cut to the column; the hint keeps the whole path.
            hint: path.map_or_else(
                || NO_STATISTICS.into(),
                |path| format!("The project's checkout: {path}"),
            ),
            mono: path.is_some(),
            test_id: None,
        },
        ProjectFact {
            label: "PM",
            value: word(pm, "in scope", "outside scope"),
            hint: match pm {
                Some(true) => "The workspace PM's approved scope includes this project.",
                Some(false) => "The workspace PM's approved scope does not include this project.",
                None => NO_STATISTICS,
            }
            .into(),
            mono: false,
            test_id: None,
        },
        ProjectFact {
            label: "Autopilot",
            value: word(autopilot, "on", "off"),
            hint: match autopilot {
                Some(true) => "An active autopilot grant names this project.",
                Some(false) => "No autopilot grant names this project.",
                None => NO_STATISTICS,
            }
            .into(),
            mono: false,
            test_id: None,
        },
        count(
            "open",
            "Open",
            open,
            "Tasks that are neither done nor dropped",
        ),
        count(
            "running",
            "Running",
            running,
            "Open tasks with a live worker session",
        ),
    ]
}

pub fn page(feed: &Feed, id: &Id) -> Option<ProjectPage> {
    if !contains_project(feed, id) {
        return None;
    }
    let row = feed
        .projects
        .get(id)
        .filter(|_| feed.capable(corral_protocol::PROJECTS_V1));
    let plans = feed
        .plans
        .values()
        .filter(|plan| plan.projects.iter().any(|name| name == id.as_str()))
        .map(|plan| {
            let (state, hue) = plan_chip(plan.state);
            let plan_id = Id::from(&plan.id);
            ProjectPlan {
                open: Press::Open(Live::Plan(plan_id.clone())),
                id: plan_id,
                title: plan.title.clone(),
                state,
                hue,
            }
        })
        .collect();
    let mut builds = feed
        .builds
        .values()
        .filter(|build| build.project.as_deref() == Some(id.as_str()))
        .collect::<Vec<_>>();
    builds.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
    let builds = builds
        .into_iter()
        .take(10)
        .map(|build| ProjectBuild {
            id: Id::from(&build.id),
            title: build.title.clone(),
            state: build.state.clone(),
        })
        .collect();
    let idle = row.map_or_else(
        || {
            !feed
                .board
                .tasks()
                .iter()
                .any(|task| is_open(task) && task.project.as_deref() == Some(id.as_str()))
        },
        |row| row.open == 0,
    );
    let empty = idle.then_some(ProjectEmpty {
        heading: "Not managed yet",
        body: "Ask to file work for it. Tasks, plans and builds filed for this project show here.",
    });
    Some(ProjectPage {
        id: format!("project-{id}"),
        project: id.clone(),
        title: id.to_string(),
        facts: facts(
            row.map(|row| row.path.as_str()),
            row.map(|row| row.in_pm_scope),
            row.map(|row| row.autopilot),
            row.map(|row| row.open),
            row.map(|row| row.running),
        ),
        plans,
        sessions: sessions(feed, id),
        builds,
        burnup_request: DetailRef::Burnup {
            scope: BurnupScope::Project(id.to_string()).key(),
        },
        empty,
        open_now_label: "Open in Now",
        ask_label: "Ask",
        ask_hint: "Ask about this project; filed ideas land in it",
    })
}
