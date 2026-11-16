//! Local project pages reuse served summaries; opening one requires no new daemon RPC.
use super::{ProjectFilter, contains_project, is_open};
use crate::model::{
    Hue, Id,
    daemon::{Feed, Group},
    timeline::BurnupScope,
};
use corral_protocol::assistant::ContextRef;
use corral_protocol::detail::DetailRef;

pub const TASKS_HEADING: &str = "Open tasks";
pub const PLANS_HEADING: &str = "Plans";
pub const BUILDS_HEADING: &str = "Recent builds";
pub const NO_PLANS: &str = "No plan names this project.";
pub const NO_BUILDS: &str = "No published builds for this project.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTask {
    pub id: Id,
    pub title: String,
    pub state: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTaskGroup {
    pub label: &'static str,
    pub hue: Hue,
    pub tasks: Vec<ProjectTask>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPlan {
    pub id: Id,
    pub title: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectBuild {
    pub id: Id,
    pub title: String,
    pub state: String,
}
/// Said in place of the task groups while the project has no open work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectEmpty {
    pub heading: &'static str,
    pub body: &'static str,
}
/// One of the page's four figures; `None` when corrald does not report project statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectCount {
    pub slug: &'static str,
    pub label: &'static str,
    pub value: Option<u32>,
    pub hue: Hue,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPage {
    pub id: String,
    pub project: Id,
    pub title: String,
    pub path: Option<String>,
    pub in_pm_scope: Option<bool>,
    pub autopilot: Option<bool>,
    pub pm_label: String,
    pub autopilot_label: String,
    pub open: Option<u32>,
    pub running: Option<u32>,
    pub review: Option<u32>,
    pub needs_me: Option<u32>,
    pub tasks: Vec<ProjectTaskGroup>,
    pub plans: Vec<ProjectPlan>,
    pub builds: Vec<ProjectBuild>,
    pub burnup_scope: BurnupScope,
    pub burnup_request: DetailRef,
    pub empty: Option<ProjectEmpty>,
    pub open_now_label: &'static str,
    pub open_now_filter: ProjectFilter,
    pub ask_label: &'static str,
    pub ask_context: ContextRef,
}

impl ProjectPage {
    /// Scope and autopilot as chips: on in the passing hue, off or unknown neutral.
    pub fn state_chips(&self) -> [(String, Hue); 2] {
        let hue = |on: Option<bool>| {
            if on == Some(true) {
                Hue::Ok
            } else {
                Hue::Neutral
            }
        };
        [
            (self.pm_label.clone(), hue(self.in_pm_scope)),
            (self.autopilot_label.clone(), hue(self.autopilot)),
        ]
    }

    pub fn counts(&self) -> [ProjectCount; 4] {
        let needs = if self.needs_me.unwrap_or(0) > 0 {
            Hue::You
        } else {
            Hue::Neutral
        };
        [
            ProjectCount {
                slug: "open",
                label: "Open",
                value: self.open,
                hue: Hue::Neutral,
            },
            ProjectCount {
                slug: "running",
                label: "Running",
                value: self.running,
                hue: Hue::Worker,
            },
            ProjectCount {
                slug: "review",
                label: "Review",
                value: self.review,
                hue: Hue::Corral,
            },
            ProjectCount {
                slug: "needs-me",
                label: "Needs me",
                value: self.needs_me,
                hue: needs,
            },
        ]
    }
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

pub fn page(feed: &Feed, id: &Id) -> Option<ProjectPage> {
    if !contains_project(feed, id) {
        return None;
    }
    let row = feed
        .projects
        .get(id)
        .filter(|_| feed.capable(corral_protocol::PROJECTS_V1));
    let order = [
        ("Needs you", Hue::You, &[Group::NeedsYou][..]),
        ("Verifying", Hue::Corral, &[Group::Verifying][..]),
        ("Producing", Hue::Worker, &[Group::Producing][..]),
        ("Upcoming", Hue::Neutral, &[Group::Queued, Group::Other][..]),
        ("Held", Hue::Held, &[Group::Held][..]),
        ("Parked", Hue::Neutral, &[Group::Parked][..]),
    ];
    let tasks = order
        .into_iter()
        .filter_map(|(label, hue, groups)| {
            let tasks = feed
                .board
                .tasks()
                .iter()
                .filter(|task| {
                    is_open(task)
                        && task.project.as_deref() == Some(id.as_str())
                        && groups.contains(&task.group)
                })
                .map(|task| ProjectTask {
                    id: task.id.clone(),
                    title: task.title.clone(),
                    state: task.now.clone(),
                })
                .collect::<Vec<_>>();
            (!tasks.is_empty()).then_some(ProjectTaskGroup { label, hue, tasks })
        })
        .collect::<Vec<_>>();
    let plans = feed
        .plans
        .values()
        .filter(|plan| plan.projects.iter().any(|name| name == id.as_str()))
        .map(|plan| ProjectPlan {
            id: Id::from(&plan.id),
            title: plan.title.clone(),
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
    let burnup_scope = BurnupScope::Project(id.to_string());
    let empty = row.map_or(tasks.is_empty(), |row| row.open == 0).then_some(ProjectEmpty {
        heading: "Not managed yet",
        body: "Ask to file work for it. Tasks, plans and builds filed for this project show here.",
    });
    Some(ProjectPage {
        id: format!("project-{id}"),
        project: id.clone(),
        title: id.to_string(),
        path: row.map(|row| row.path.clone()),
        in_pm_scope: row.map(|row| row.in_pm_scope),
        autopilot: row.map(|row| row.autopilot),
        pm_label: row
            .map_or("PM scope unknown", |row| {
                if row.in_pm_scope {
                    "PM in scope"
                } else {
                    "PM out of scope"
                }
            })
            .into(),
        autopilot_label: row
            .map_or("Autopilot unknown", |row| {
                if row.autopilot {
                    "Autopilot on"
                } else {
                    "Autopilot off"
                }
            })
            .into(),
        open: row.map(|row| row.open),
        running: row.map(|row| row.running),
        review: row.map(|row| row.review),
        needs_me: row.map(|row| row.needs_me),
        tasks,
        plans,
        builds,
        burnup_request: DetailRef::Burnup {
            scope: burnup_scope.key(),
        },
        burnup_scope,
        empty,
        open_now_label: "Open in Now",
        open_now_filter: ProjectFilter::Project(id.clone()),
        ask_label: "Ask about this project",
        ask_context: ContextRef {
            kind: "project".into(),
            id: id.to_string(),
            rev: None,
        },
    })
}
