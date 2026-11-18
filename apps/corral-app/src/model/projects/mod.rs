//! Workspace projects and independent, device-local screen scopes. No client or UI runtime.
mod filters;
mod page;
#[cfg(test)]
mod tests;

pub use filters::{FilterScreen, Filters, NowGroup, NowScope};
pub use page::{
    FACTS_LABEL, PLANS_HEADING, PLANS_HINT, ProjectEmpty, ProjectFact, ProjectPage, ProjectPlan,
    ProjectSession, SESSIONS_HEADING, SESSIONS_HINT, not_found, page, project_now,
};

/// The sidebar section's and the phone list's name.
pub const PROJECTS_HEADING: &str = "Projects";
/// The phone More row that opens the project list.
pub const PROJECTS_MORE_SUBTITLE: &str = "Open work by project";

use crate::model::{
    Id,
    daemon::{Feed, Group, Task},
};
use corral_types::Status;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;

pub(crate) fn is_open(task: &Task) -> bool {
    !matches!(task.status(), Status::Done | Status::Dropped)
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
            .any(|task| is_open(task) && task.project() == Some(id))
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
            .filter_map(Task::project)
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
        .filter_map(Task::project);
    let first = names.next();
    names.any(|name| Some(name) != first)
}

pub fn has_unassigned(feed: &Feed) -> bool {
    feed.board
        .tasks()
        .iter()
        .any(|task| is_open(task) && task.project().is_none())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEntry {
    /// Sidebar/More interaction id, including the required project prefix.
    pub id: String,
    pub label: String,
    pub filter: ProjectFilter,
    /// Unknown on old daemons; never synthesize canonical statistics from partial lists.
    pub open: Option<u32>,
    /// Counted from the complete open-task list; unknown until it arrives.
    pub running: Option<u32>,
    pub review: Option<u32>,
    pub needs_me: Option<u32>,
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
            return UNASSIGNED_HINT.into();
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

    /// What the sidebar's needs-you figure stands for: `3 wait on you`; `None` while nothing
    /// waits or the daemon reports no counts.
    pub fn needs_hint(&self) -> Option<String> {
        self.needs_me
            .filter(|n| *n > 0)
            .map(crate::model::health::waits_on_you)
    }

    /// What the sidebar's open figure stands for: `40 open tasks: 2 running, 5 in review`;
    /// `None` where the daemon reports no counts.
    pub fn open_hint(&self) -> Option<String> {
        let open = self.open?;
        let noun = if open == 1 { "task" } else { "tasks" };
        let detail = [(self.running, "running"), (self.review, "in review")]
            .into_iter()
            .filter_map(|(n, word)| n.filter(|n| *n > 0).map(|n| format!("{n} {word}")))
            .collect::<Vec<_>>();
        Some(if detail.is_empty() {
            format!("{open} open {noun}")
        } else {
            format!("{open} open {noun}: {}", detail.join(", "))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projects {
    pub entries: Vec<ProjectEntry>,
}

/// How many of the project's open tasks are in `group`, once the open-task list is complete.
fn count(feed: &Feed, id: &str, groups: &[Group]) -> Option<u32> {
    feed.board.complete.then(|| {
        let count = feed
            .board
            .tasks()
            .iter()
            .filter(|task| task.project() == Some(id) && groups.contains(&task.group))
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    })
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
                    open: row.map(|row| u32::try_from(row.open).unwrap_or(u32::MAX)),
                    running: count(feed, &id, &[Group::Producing]),
                    review: count(feed, &id, &[Group::Verifying]),
                    needs_me: count(feed, &id, &[Group::NeedsYou]),
                    filter: ProjectFilter::Project(id),
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
                selected: filter == ProjectFilter::Unassigned,
            });
        }
        Self { entries }
    }
}

/// One scope of a screen's project bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterChip {
    pub id: String,
    pub label: String,
    pub filter: ProjectFilter,
    pub selected: bool,
    /// What waits on the owner in this project, as figures (`3`): none when nothing does, for
    /// All and Unassigned, and on daemons that report no project counts.
    pub needs: Option<String>,
    /// The sentence the chip stands for: `3 wait on you in corral · 5 open · 2 running`.
    pub hint: String,
    /// What a screen reader hears for the toggle: its name, then what waits on the owner.
    pub spoken: String,
}

const ALL_HINT: &str = "Every project in this workspace";
const UNASSIGNED_HINT: &str = "Tasks filed without a project";

impl FilterChip {
    fn all(selected: bool) -> Self {
        Self {
            id: "filter-all".into(),
            label: "All".into(),
            filter: ProjectFilter::All,
            selected,
            needs: None,
            hint: ALL_HINT.into(),
            spoken: "All".into(),
        }
    }

    fn of(entry: ProjectEntry) -> Self {
        let waiting = entry.needs_me.filter(|n| *n > 0);
        let needs_line =
            waiting.map(|n| format!("{n} {} on you", if n == 1 { "waits" } else { "wait" }));
        let hint = match entry.filter {
            ProjectFilter::Project(_) => {
                let mut parts = Vec::new();
                if let Some(line) = &needs_line {
                    parts.push(format!("{line} in {}", entry.label));
                }
                parts.push(match (entry.open, entry.running) {
                    (None, _) => "Counts not reported by this corrald".to_owned(),
                    (Some(0), _) => "No open work".to_owned(),
                    (Some(open), Some(running)) if running > 0 => {
                        format!("{open} open · {running} running")
                    }
                    (Some(open), _) => format!("{open} open"),
                });
                parts.join(" · ")
            }
            ProjectFilter::All | ProjectFilter::Unassigned => entry.summary(),
        };
        let spoken = match &needs_line {
            Some(line) => format!("{}, {line}", entry.label),
            None => entry.label.clone(),
        };
        Self {
            id: format!("filter-{}", entry.id),
            label: entry.label,
            filter: entry.filter,
            selected: entry.selected,
            needs: waiting.map(|n| n.to_string()),
            hint,
            spoken,
        }
    }
}

/// Empty means the screen should omit its project chip strip entirely.
pub fn chips(feed: &Feed, filter: &ProjectFilter, screen: FilterScreen) -> Vec<FilterChip> {
    // Legacy daemons send no plan projects, so the Plans builders ignore any filter there.
    if screen == FilterScreen::Plans && !feed.capable(corral_protocol::PROJECTS_V1) {
        return Vec::new();
    }
    if *filter == ProjectFilter::All && !has_multiple_projects(feed) && !has_unassigned(feed) {
        return Vec::new();
    }
    let mut chips = vec![FilterChip::all(filter.resolved(feed) == ProjectFilter::All)];
    chips.extend(
        Projects::build(feed, filter)
            .entries
            .into_iter()
            .map(FilterChip::of),
    );
    if *filter == ProjectFilter::Unassigned && !has_unassigned(feed) {
        chips.push(FilterChip {
            id: "filter-filter-unassigned".into(),
            label: "Unassigned".into(),
            filter: ProjectFilter::Unassigned,
            selected: true,
            needs: None,
            hint: UNASSIGNED_HINT.into(),
            spoken: "Unassigned".into(),
        });
    }
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
