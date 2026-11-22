//! Activity › System's state: the owner's actions on the map, and Escape, which clears a
//! typed filter, then goes up a level, then lets go of the pick.

use crate::model::system::Action;

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
