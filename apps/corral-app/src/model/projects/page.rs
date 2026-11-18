//! Local project pages reuse served summaries; opening one requires no new daemon RPC. The page
//! is Now scoped to one project ([`project_now`]) beside the project's own facts, with its
//! plans, its sessions and its burn-up.
use std::borrow::Cow;

use corral_protocol::detail::DetailRef;
use corral_protocol::rows::{AgentRow, PlanRow};
use corral_types::{SessionStatus, Status};
use serde_json::{Map, Value};

use super::{NowGroup, ProjectFilter, contains_project, is_open};
use crate::model::work::{self, NowPage, Press};
use crate::model::{Form, Hue, Id, Live, Selection, daemon::Feed, timeline::BurnupScope};

pub const PLANS_HEADING: &str = "Plans";
pub const PLANS_HINT: &str = "Plans (parent tasks) in this project. A row opens the plan.";
pub const SESSIONS_HEADING: &str = "Sessions";
pub const SESSIONS_HINT: &str = "Live runs on this project's tasks. A row opens the session.";
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
/// A plan in the project: its id, title and state chip; a press opens it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPlan {
    pub id: Id,
    pub title: String,
    pub state: &'static str,
    pub hue: Hue,
    pub open: Press,
}
/// A live run on one of the project's tasks: its id, name and state chip; what it is doing
/// in its hint; a press opens it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSession {
    pub id: Id,
    /// The id the row shows: the run's task.
    pub shown_id: String,
    pub name: String,
    pub state: &'static str,
    pub hue: Hue,
    pub doing: String,
    pub open: Press,
}
/// Said in place of the moving tasks while the project has no open work.
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

/// Now's page in the project's scope, grouped by who moves next, with nothing selected: the
/// page has no side panel of its own, so a row opens its task and the side column always holds
/// the project's digest.
pub fn project_now(state: &work::State, feed: &Feed, form: Form, project: &Id) -> NowPage {
    // A selection made on Now must not take this page's side column from the digest.
    let state = if state.digest_only {
        Cow::Borrowed(state)
    } else {
        Cow::Owned(work::State {
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
    for task in page
        .sections
        .iter_mut()
        .flat_map(|section| section.rows.iter_mut())
        .chain(
            page.waiting
                .groups
                .iter_mut()
                .flat_map(|group| group.rows.iter_mut()),
        )
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

/// A plan's chip from its children: one blocked needs the owner; any running or in review is
/// executing; all done is complete.
fn plan_chip(plan: &PlanRow) -> (&'static str, Hue) {
    let has = |statuses: &[Status]| {
        plan.child_counts
            .iter()
            .any(|count| count.count > 0 && statuses.contains(&count.status))
    };
    if has(&[Status::Blocked]) {
        ("needs you", Hue::You)
    } else if has(&[Status::Running, Status::Review]) {
        ("executing", Hue::Corral)
    } else if !has(&[
        Status::Idea,
        Status::Backlog,
        Status::Ready,
        Status::Running,
        Status::Blocked,
        Status::Review,
    ]) && has(&[Status::Done])
    {
        ("complete", Hue::Ok)
    } else {
        ("waiting", Hue::Neutral)
    }
}

/// A live run's state chip.
fn session_chip(row: &AgentRow) -> (&'static str, Hue) {
    match row.status {
        SessionStatus::Working => ("working", Hue::Worker),
        SessionStatus::Blocked { .. } => ("needs you", Hue::You),
        SessionStatus::Idle => ("idle", Hue::Neutral),
        SessionStatus::Gone => ("gone", Hue::Fail),
    }
}

/// The project's live runs: those whose task is open under this project, blocked ones first.
fn sessions(feed: &Feed, id: &Id) -> Vec<ProjectSession> {
    let mut rows: Vec<_> = feed
        .agents
        .iter()
        .filter(|(_, row)| {
            feed.board
                .task(&row.run.task)
                .is_some_and(|task| task.project() == Some(id.as_str()))
        })
        .collect();
    rows.sort_by_key(|(key, row)| {
        (
            !matches!(row.status, SessionStatus::Blocked { .. }),
            (*key).clone(),
        )
    });
    rows.into_iter()
        .map(|(key, row)| {
            let (state, hue) = session_chip(row);
            let run = &row.run;
            ProjectSession {
                open: Press::Open(Live::Session(key.clone())),
                id: key.clone(),
                shown_id: run.task.clone(),
                name: format!("{} · {} · {}", run.id, run.role.as_str(), run.profile),
                state,
                hue,
                doing: match &row.status {
                    SessionStatus::Blocked { excerpt } => excerpt.clone(),
                    _ => run
                        .summary
                        .clone()
                        .unwrap_or_else(|| "Doing now unknown".into()),
                },
            }
        })
        .collect()
}

/// A project switch: the project's own setting, else the workspace default from Settings.
fn switch(project: Option<&Map<String, Value>>, feed: &Feed, key: &str) -> Option<bool> {
    project
        .and_then(|config| config.get(key))
        .and_then(Value::as_bool)
        .or_else(|| {
            feed.settings
                .as_ref()?
                .config
                .get("defaults")?
                .get(key)?
                .as_bool()
        })
}

/// Path, the two automation switches, open and running, each unknown while corrald does not
/// report it.
fn facts(
    path: Option<&str>,
    dispatch: Option<bool>,
    land: Option<bool>,
    open: Option<u64>,
    running: Option<u64>,
) -> Vec<ProjectFact> {
    let switch = |label, known: Option<bool>, on: &str, off: &str| ProjectFact {
        label,
        value: known
            .map_or(UNKNOWN, |known| if known { "on" } else { "off" })
            .to_owned(),
        hint: match known {
            Some(true) => on.to_owned(),
            Some(false) => off.to_owned(),
            None => NO_STATISTICS.to_owned(),
        },
        mono: false,
        test_id: None,
    };
    let count = |slug: &str, label: &'static str, value: Option<u64>, hint: &str| ProjectFact {
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
        switch(
            "Auto dispatch",
            dispatch,
            "Ready tasks are dispatched automatically.",
            "Ready tasks wait for a dispatch.",
        ),
        switch(
            "Auto land",
            land,
            "Reviewed tasks land automatically.",
            "Reviewed tasks wait for Approve and land.",
        ),
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
            "Open tasks with a running worker",
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
    let config = row.map(|row| &row.config);
    let plans = feed
        .plans
        .values()
        .filter(|plan| plan.parent.project.as_deref() == Some(id.as_str()))
        .map(|plan| {
            let (state, hue) = plan_chip(plan);
            let plan_id = Id::new(&plan.parent.id);
            ProjectPlan {
                open: Press::Open(Live::Plan(plan_id.clone())),
                id: plan_id,
                title: plan.parent.title.clone(),
                state,
                hue,
            }
        })
        .collect();
    let mine = || {
        feed.board
            .tasks()
            .iter()
            .filter(|task| is_open(task) && task.project() == Some(id.as_str()))
    };
    let idle = row.map_or_else(|| mine().next().is_none(), |row| row.open == 0);
    let running = feed.board.complete.then(|| {
        mine()
            .filter(|task| task.status() == Status::Running)
            .count() as u64
    });
    let empty = idle.then_some(ProjectEmpty {
        heading: "Not managed yet",
        body: "Ask to file work for it. Tasks and plans filed for this project show here.",
    });
    Some(ProjectPage {
        id: format!("project-{id}"),
        project: id.clone(),
        title: id.to_string(),
        facts: facts(
            config.and_then(|config| config.get("path")?.as_str()),
            switch(config, feed, "auto_dispatch"),
            switch(config, feed, "auto_land"),
            row.map(|row| row.open),
            running,
        ),
        plans,
        sessions: sessions(feed, id),
        burnup_request: DetailRef::Burnup {
            scope: BurnupScope::Project(id.to_string()).key(),
        },
        empty,
        open_now_label: "Open in Now",
        ask_label: "Ask",
        ask_hint: "Ask about this project; filed ideas land in it",
    })
}
