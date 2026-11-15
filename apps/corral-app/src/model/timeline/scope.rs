//! Goal burn-up selectors use the daemon's scopes, independently of the timeline date window.

use std::collections::BTreeSet;

use corral_protocol::detail::DetailRef;

use super::State;
use crate::model::daemon::Feed;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum BurnupScope {
    #[default]
    Workspace,
    Project(String),
    Plan(String),
}

impl BurnupScope {
    pub fn key(&self) -> String {
        match self {
            Self::Workspace => "workspace".into(),
            Self::Project(name) => format!("project:{name}"),
            Self::Plan(id) => format!("plan:{id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeOption {
    pub id: String,
    pub label: String,
    pub scope: BurnupScope,
    pub selected: bool,
}

pub fn scope_options(state: &State, feed: &Feed) -> Vec<ScopeOption> {
    let mut scopes = BTreeSet::from([BurnupScope::Workspace, state.burnup_scope.clone()]);
    scopes.extend(
        feed.board
            .tasks()
            .iter()
            .filter_map(|task| task.project.clone())
            .map(BurnupScope::Project),
    );
    scopes.extend(
        feed.plans
            .keys()
            .map(|id| BurnupScope::Plan(id.to_string())),
    );
    scopes.extend(
        feed.board
            .tasks()
            .iter()
            .filter_map(|task| task.plan.clone())
            .map(BurnupScope::Plan),
    );
    scopes
        .into_iter()
        .map(|scope| ScopeOption {
            id: scope.key(),
            label: match &scope {
                BurnupScope::Workspace => "Workspace".into(),
                BurnupScope::Project(name) => format!("Project · {name}"),
                BurnupScope::Plan(id) => format!("Plan · {id}"),
            },
            selected: scope == state.burnup_scope,
            scope,
        })
        .collect()
}

pub fn burnup_request(state: &State) -> DetailRef {
    DetailRef::Burnup {
        scope: state.burnup_scope.key(),
    }
}
