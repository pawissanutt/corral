//! Insight's System map state: the owner's actions on the map, and Escape, which clears a
//! typed filter, then goes up a level, then lets go of the pick.

use corral_types::SystemAt;

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
    /// An action on task `task`'s Impact; another task's pick is let go first.
    pub fn impact_act(&self, task: &str, action: crate::model::review::impact::Action) {
        self.impact.update(|state| {
            state.follow(task);
            state.act(action);
        });
    }

    /// Insight › Structure scoped to `project` (the one scope every place shares, as the
    /// System page's own project picker sets it), at `trail` with `node` selected: a place
    /// `impact::reach` found on that page's own map.
    pub fn open_in_system(&self, project: Id, trail: Vec<SystemAt>, node: Node) {
        self.set_scope(Scope::Project(project.clone()));
        self.system.update(|state| {
            state.follow(project.as_str());
            state.view = crate::model::system::View::Structure;
            state.trail = trail;
            state.filter.clear();
            state.picked = Some(Pick::Node(node));
        });
        self.go(Destination::Structure);
    }
}
