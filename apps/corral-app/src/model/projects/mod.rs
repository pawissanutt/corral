//! Workspace projects and the one device-local project scope. No client or UI runtime.
mod branches;
mod scope;
#[cfg(test)]
mod tests;

pub use branches::{BRANCHES_CAPTION, BranchPage, branch_page, branches, branches_ref, suggest};
pub use scope::{ALL_PROJECTS, SCOPE_HEADING, Scope, ScopeEntry, Scopes, scope_menu, scope_note};

use crate::model::{
    Id,
    daemon::{Feed, Task},
};
use corral_types::Status;
use std::collections::BTreeSet;

pub(crate) fn is_open(task: &Task) -> bool {
    !matches!(task.status(), Status::Done | Status::Dropped)
}

/// The scope as the domains' `*_filtered` builders take it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProjectFilter {
    #[default]
    All,
    Project(Id),
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
        }
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
