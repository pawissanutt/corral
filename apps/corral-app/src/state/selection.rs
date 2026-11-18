//! Escape on a page with a selection: the selection closes before the page is left (the mock's
//! Esc), so the owner never loses the page they were reading to dismiss its side panel.

use crate::model::{Destination, agents, work};

use super::AppState;

impl AppState {
    /// Clears the Agents roster's selection or the Now page's while that page is in view (no
    /// page pushed over it); whether there was one to clear.
    pub(super) fn clear_page_selection(&self) -> bool {
        let (destination, bare, task) = self.nav.with_untracked(|nav| {
            (
                nav.destination,
                nav.pages.is_empty(),
                nav.selection.task.is_some(),
            )
        });
        if !bare {
            return false;
        }
        match destination {
            Destination::Agents
                if self
                    .agents
                    .with_untracked(|state| state.roster_session.is_some()) =>
            {
                self.reduce(self.agents, |state, feed, _, env| {
                    state.act_roster(agents::Action::ClearRoster, feed, env)
                });
                true
            }
            Destination::Now
                if task && self.work.with_untracked(|state| !state.digest_only) =>
            {
                self.reduce(self.work, |state, feed, _, env| {
                    state.press(work::Press::ClearSelection, feed, env)
                });
                true
            }
            _ => false,
        }
    }
}
