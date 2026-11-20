//! The one project scope of the whole app: All projects, or one. It applies to Home, Tasks,
//! Sessions and Activity together and is kept per daemon store, so several daemons keep
//! separate scopes. The `corral.scope` payload is store → `{ "project": null | "<id>" }`.
use super::{ProjectFilter, contains_project};
use crate::model::{
    Id,
    daemon::{Feed, Group},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

/// The switcher's words for no scope.
pub const ALL_PROJECTS: &str = "All projects";
/// The scope menu's heading.
pub const SCOPE_HEADING: &str = "Scope: applies to every page";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    All,
    Project(Id),
}

impl Scope {
    /// A project the daemon no longer lists never strands every page on an empty scope.
    pub fn resolved(&self, feed: &Feed) -> Scope {
        match self {
            Scope::Project(id) if !contains_project(feed, id) => Scope::All,
            _ => self.clone(),
        }
    }

    pub fn project(&self) -> Option<&Id> {
        match self {
            Scope::All => None,
            Scope::Project(id) => Some(id),
        }
    }

    /// Whether a record of `project` is in scope.
    pub fn matches(&self, project: Option<&str>) -> bool {
        match self {
            Scope::All => true,
            Scope::Project(id) => project == Some(id.as_str()),
        }
    }

    /// The scope as the filter the domains' `*_filtered` builders take.
    pub fn filter(&self) -> ProjectFilter {
        match self {
            Scope::All => ProjectFilter::All,
            Scope::Project(id) => ProjectFilter::Project(id.clone()),
        }
    }

    /// The switcher's words: the project's name, or [`ALL_PROJECTS`].
    pub fn label(&self) -> String {
        match self {
            Scope::All => ALL_PROJECTS.into(),
            Scope::Project(id) => id.to_string(),
        }
    }
}

// Null is All; a configured project's name is a nonempty string, "all" included.
impl Serialize for Scope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Scope::All => serializer.serialize_none(),
            Scope::Project(id) => serializer.serialize_str(id.as_str()),
        }
    }
}
impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<String>::deserialize(deserializer)? {
            Some(name) if !name.is_empty() => Scope::Project(Id::from(name)),
            _ => Scope::All,
        })
    }
}

/// Unknown keys (the former `now.group` and `agents.show` among them) are ignored on read and
/// dropped on the next write.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoreScope {
    project: Scope,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Scopes(BTreeMap<String, StoreScope>);
impl Scopes {
    pub fn parse(value: &str) -> Option<Self> {
        serde_json::from_str(value).ok()
    }
    pub fn encode(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
    pub fn get(&self, store: &str, feed: &Feed) -> Scope {
        self.0
            .get(store)
            .map(|saved| saved.project.resolved(feed))
            .unwrap_or_default()
    }
    pub fn set(&mut self, store: &str, scope: Scope) {
        self.0.entry(store.to_owned()).or_default().project = scope;
    }
}

/// One row of the scope menu: All projects, or a project with its open-task count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeEntry {
    /// `scope-all`, `scope-<project>`.
    pub id: String,
    pub label: String,
    pub scope: Scope,
    /// `12 open`; absent for All projects and where the daemon reports no counts.
    pub note: Option<String>,
    /// What waits on the owner in the project, when anything does.
    pub needs: Option<u32>,
    pub selected: bool,
}

/// The scope menu: All projects, then each project the daemon lists.
pub fn scope_menu(feed: &Feed, scope: &Scope) -> Vec<ScopeEntry> {
    let scope = scope.resolved(feed);
    let mut entries = vec![ScopeEntry {
        id: "scope-all".into(),
        label: ALL_PROJECTS.into(),
        scope: Scope::All,
        note: None,
        needs: None,
        selected: scope == Scope::All,
    }];
    entries.extend(super::known_projects(feed).into_iter().map(|id| {
        let open = feed
            .projects
            .get(&id)
            .filter(|_| feed.capable(corral_protocol::PROJECTS_V1))
            .map(|row| row.open);
        ScopeEntry {
            id: format!("scope-{id}"),
            label: id.to_string(),
            note: open.map(|open| format!("{open} open")),
            needs: needs_you(feed, &id).filter(|n| *n > 0),
            selected: scope == Scope::Project(id.clone()),
            scope: Scope::Project(id),
        }
    }));
    entries
}

/// How many of the project's open tasks wait on the owner, once the open-task list is
/// complete.
fn needs_you(feed: &Feed, id: &str) -> Option<u32> {
    feed.board.complete.then(|| {
        let count = feed
            .board
            .tasks()
            .iter()
            .filter(|task| task.project() == Some(id) && task.group == Group::NeedsYou)
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    })
}

/// What the switcher says under the scope: how many projects All covers, or that one is in
/// scope.
pub fn scope_note(feed: &Feed, scope: &Scope) -> String {
    match scope.resolved(feed) {
        Scope::All => match super::known_projects(feed).len() {
            1 => "1 project".into(),
            n => format!("{n} projects"),
        },
        Scope::Project(_) => "project in scope".into(),
    }
}
