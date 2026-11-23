//! Activity › System's state: the owner's actions on the map, and Escape, which clears a
//! typed filter, then goes up a level, then lets go of the pick.

use crate::model::projects::Scope;
use crate::model::system::{Action, Node, Pick};
use crate::model::{Destination, Id};

use super::AppState;

impl AppState {
    pub fn system_act(&self, action: Action) {
        self.system.update(|state| {
            state.act(action);
        });
    }

    /// Escape while the map is the page: whether it did anything.
    pub(super) fn escape_system(&self) -> bool {
        let mut done = false;
        self.system.update(|state| {
            done = if state.filter.is_empty() {
                state.act(Action::Up)
            } else {
                state.act(Action::Filter(String::new()))
            };
        });
        done
    }
}

impl AppState {
    pub fn impact_act(&self, action: crate::model::review::impact::Action) {
        self.impact.update(|state| state.act(action));
    }

    /// Activity › System scoped to `project`, at its project level with `node` selected.
    pub fn open_in_system(&self, project: Id, node: Option<Node>) {
        self.set_scope(Scope::Project(project.clone()));
        self.system.update(|state| {
            state.follow(project.as_str());
            state.trail.clear();
            state.filter.clear();
            state.picked = node.map(Pick::Node);
        });
        self.go(Destination::System);
    }
}
