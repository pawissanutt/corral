//! The artifact finder's state: its reducer runs against the selected feed, and Escape backs
//! out of full size, then clears a typed filter, before anything else it would do.

use crate::model::Id;
use crate::model::artifacts::Action;

use super::AppState;

impl AppState {
    /// Runs one finder action against the selected feed and applies its effects.
    pub fn artifacts_act(&self, action: Action) {
        let mut effects = Vec::new();
        self.feed.with_untracked(|feed| {
            self.artifacts
                .update(|state| effects = state.act(action, feed));
        });
        self.run(effects);
    }

    /// Opens what a citation names, as spelled in the text (`T-12:3#goal`); one that does not
    /// resolve says so.
    pub fn open_cite(&self, text: &str) {
        let effects = self
            .feed
            .with_untracked(|feed| crate::model::cite::press_text(feed, text));
        // Ask draws the same transcript over the page; what a citation opens shows beneath it.
        let opens = effects
            .iter()
            .any(|effect| !matches!(effect, crate::model::Effect::Toast(_)));
        if opens {
            if self.dock_panel().is_some() {
                self.dismiss_dock_overlay();
            } else if self.is_open(crate::model::Overlay::Ask) {
                self.close(crate::model::Overlay::Ask);
            }
        }
        self.run(effects);
    }

    /// Opens the finder with `task`'s token filled in.
    pub(super) fn find_artifacts(&self, task: Id) {
        self.artifacts_act(Action::Find(task));
    }

    /// Escape while the finder is the page: whether it backed out of full size or cleared the
    /// filter.
    pub(super) fn escape_artifacts(&self) -> bool {
        let mut done = false;
        self.artifacts.update(|state| done = state.escape());
        done
    }
}
