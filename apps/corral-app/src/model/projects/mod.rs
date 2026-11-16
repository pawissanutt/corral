//! Workspace projects and independent, device-local screen scopes. No client or UI runtime.
mod filters;
mod page;
#[cfg(test)]
mod tests;

pub use filters::{FilterScreen, Filters, NowGroup, NowScope};
pub use page::{
    BUILDS_HEADING, NO_BUILDS, NO_PLANS, PLANS_HEADING, ProjectBuild, ProjectCount, ProjectEmpty,
    ProjectPage, ProjectPlan, ProjectTask, ProjectTaskGroup, TASKS_HEADING, not_found, page,
};

/// The sidebar section's and the phone list's name.
pub const PROJECTS_HEADING: &str = "Projects";
/// The phone More row that opens the project list.
pub const PROJECTS_MORE_SUBTITLE: &str = "Open work by project";

use crate::model::{
    Id,
    daemon::{Feed, Task},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;

pub(crate) fn is_open(task: &Task) -> bool {
    !matches!(
        task.now_code.as_str(),
        "done" | "landed" | "dropped" | "cancelled"
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProjectFilter {
    #[default]
    All,
    Project(Id),
    Unassigned,
}

impl ProjectFilter {
    /// Missing projects never strand a list on an impossible scope.
    pub fn resolved(&self, feed: &Feed) -> Self {
        match self {
            Self::Project(id) if !contains_project(feed, id) => Self::All,
            Self::Unassigned if !has_unassigned(feed) => Self::All,
            _ => self.clone(),
        }
    }

    pub fn matches(&self, project: Option<&str>) -> bool {
        match self {
            Self::All => true,
            Self::Project(id) => project == Some(id.as_str()),
            Self::Unassigned => project.is_none(),
        }
    }
}

// Null is All and the empty name is Unassigned; configured project names are nonempty.
// Real project names remain plain strings, including names such as "all" or "unassigned".
impl Serialize for ProjectFilter {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::All => serializer.serialize_none(),
            Self::Project(id) => serializer.serialize_str(id.as_str()),
            Self::Unassigned => serializer.serialize_str(""),
        }
    }
}
impl<'de> Deserialize<'de> for ProjectFilter {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<String>::deserialize(deserializer)? {
            None => Self::All,
            Some(name) if name.is_empty() => Self::Unassigned,
            Some(name) => Self::Project(Id::from(name)),
        })
    }
}

pub fn contains_project(feed: &Feed, id: &str) -> bool {
    if feed.capable(corral_protocol::PROJECTS_V1) {
        feed.projects.contains_key(id)
    } else {
        feed.board
            .tasks()
            .iter()
            .any(|task| is_open(task) && task.project.as_deref() == Some(id))
    }
}

pub fn known_projects(feed: &Feed) -> Vec<Id> {
    if feed.capable(corral_protocol::PROJECTS_V1) {
        feed.projects.keys().cloned().collect()
    } else {
        feed.board
            .tasks()
            .iter()
            .filter(|task| is_open(task))
            .filter_map(|task| task.project.as_deref())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(Id::from)
            .collect()
    }
}

pub fn has_multiple_projects(feed: &Feed) -> bool {
    if feed.capable(corral_protocol::PROJECTS_V1) {
        return feed.projects.len() >= 2;
    }
    let mut names = feed
        .board
        .tasks()
        .iter()
        .filter(|task| is_open(task))
        .filter_map(|task| task.project.as_deref());
    let first = names.next();
    names.any(|name| Some(name) != first)
}

pub fn has_unassigned(feed: &Feed) -> bool {
    feed.board
        .tasks()
        .iter()
        .any(|task| is_open(task) && task.project.is_none())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEntry {
    /// Sidebar/More interaction id, including the required project prefix.
    pub id: String,
    pub label: String,
    pub filter: ProjectFilter,
    /// Unknown on old daemons; never synthesize canonical statistics from partial lists.
    pub open: Option<u32>,
    pub running: Option<u32>,
    pub review: Option<u32>,
    pub needs_me: Option<u32>,
    pub in_pm_scope: Option<bool>,
    pub autopilot: Option<bool>,
    pub selected: bool,
}

impl ProjectEntry {
    /// The configured project this row opens; `None` for the Unassigned row.
    pub fn project(&self) -> Option<&Id> {
        match &self.filter {
            ProjectFilter::Project(id) => Some(id),
            ProjectFilter::All | ProjectFilter::Unassigned => None,
        }
    }

    /// The counts as a line: what is open, running and waiting on the owner.
    pub fn summary(&self) -> String {
        if self.filter == ProjectFilter::Unassigned {
            return "Tasks filed without a project".into();
        }
        let Some(open) = self.open else {
            return "Counts not reported by this corrald".into();
        };
        if open == 0 {
            return "No open work".into();
        }
        let mut parts = vec![format!("{open} open")];
        if let Some(running) = self.running.filter(|n| *n > 0) {
            parts.push(format!("{running} running"));
        }
        if let Some(needs) = self.needs_me.filter(|n| *n > 0) {
            parts.push(format!("{needs} need you"));
        }
        parts.join(" · ")
    }

    /// What assistive technology hears for the row: its name, then [`Self::summary`].
    pub fn spoken(&self) -> String {
        format!("{}, {}", self.label, self.summary())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projects {
    pub entries: Vec<ProjectEntry>,
}
impl Projects {
    pub fn build(feed: &Feed, filter: &ProjectFilter) -> Self {
        let filter = filter.resolved(feed);
        let mut entries = known_projects(feed)
            .into_iter()
            .map(|id| {
                let row = feed
                    .projects
                    .get(&id)
                    .filter(|_| feed.capable(corral_protocol::PROJECTS_V1));
                ProjectEntry {
                    id: format!("project-{id}"),
                    label: id.to_string(),
                    selected: filter == ProjectFilter::Project(id.clone()),
                    filter: ProjectFilter::Project(id),
                    open: row.map(|row| row.open),
                    running: row.map(|row| row.running),
                    review: row.map(|row| row.review),
                    needs_me: row.map(|row| row.needs_me),
                    in_pm_scope: row.map(|row| row.in_pm_scope),
                    autopilot: row.map(|row| row.autopilot),
                }
            })
            .collect::<Vec<_>>();
        if has_unassigned(feed) {
            entries.push(ProjectEntry {
                id: "filter-unassigned".into(),
                label: "Unassigned".into(),
                filter: ProjectFilter::Unassigned,
                open: None,
                running: None,
                review: None,
                needs_me: None,
                in_pm_scope: None,
                autopilot: None,
                selected: filter == ProjectFilter::Unassigned,
            });
        }
        Self { entries }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterChip {
    pub id: String,
    pub label: String,
    pub filter: ProjectFilter,
    pub selected: bool,
}

/// Empty means the screen should omit its project chip strip entirely.
pub fn chips(feed: &Feed, filter: &ProjectFilter, screen: FilterScreen) -> Vec<FilterChip> {
    if (screen == FilterScreen::Plans && !feed.capable(corral_protocol::PROJECTS_V1))
        || (!has_multiple_projects(feed) && !has_unassigned(feed))
    {
        return Vec::new();
    }
    let mut chips = vec![FilterChip {
        id: "filter-all".into(),
        label: "All".into(),
        filter: ProjectFilter::All,
        selected: filter.resolved(feed) == ProjectFilter::All,
    }];
    chips.extend(
        Projects::build(feed, filter)
            .entries
            .into_iter()
            .map(|entry| FilterChip {
                id: format!("filter-{}", entry.id),
                label: entry.label,
                filter: entry.filter,
                selected: entry.selected,
            }),
    );
    chips
}

pub fn project_chip(feed: &Feed, filter: &ProjectFilter, project: Option<&str>) -> Option<String> {
    (filter.resolved(feed) == ProjectFilter::All && has_multiple_projects(feed))
        .then(|| project.unwrap_or("Unassigned").to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkspaceMode {
    #[default]
    Managed,
    AssistantOnly,
}
impl WorkspaceMode {
    pub fn of(feed: &Feed) -> Self {
        let complete = feed
            .topics
            .get("projects")
            .is_some_and(|state| !state.loading && !state.stale && state.error.is_none());
        if feed.capable(corral_protocol::PROJECTS_V1) && complete && feed.projects.is_empty() {
            Self::AssistantOnly
        } else {
            Self::Managed
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantLanding {
    pub title: &'static str,
    pub explanation: &'static str,
    pub instructions_heading: &'static str,
    pub instructions: &'static str,
    pub ask_label: &'static str,
}

pub fn assistant_landing() -> AssistantLanding {
    AssistantLanding {
        title: "Ask about your workspace",
        explanation: "No projects are configured. Ask can help before project management is set up.",
        instructions_heading: "Add a project",
        instructions: "Put a git repo under the workspace root or add [projects.<name>] path = ... to .corral/config.toml.",
        ask_label: "Ask",
    }
}
