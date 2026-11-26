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
        // The conversation the citation was pressed in is kept. Pressed in the desktop's side
        // pane (Ask's tab or a session's, which mark the copy they draw with), the pane stays
        // beside the page, or steps aside over a narrow one once the page has gone there;
        // pressed in the page, the pane is left as it is. Ask over the page elsewhere closes so
        // what it opens shows.
        let opens = effects
            .iter()
            .any(|effect| !matches!(effect, crate::model::Effect::Toast(_)));
        if opens && crate::model::dock::supported(self.form) {
            if self.cites_from_pane() {
                self.open_from_pane(effects);
                return;
            }
        } else if opens && self.is_open(crate::model::Overlay::Ask) {
            self.close(crate::model::Overlay::Ask);
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
