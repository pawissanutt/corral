//! Escape on a page with a selection: the selection closes before the page is left (the mock's
//! Esc), so the owner never loses the page they were reading to dismiss its side panel. On a
//! session page Escape first returns a view to Chat, then stops the working turn, and only then
//! leaves the page.

use crate::model::session::{self, Cx, Facts};
use crate::model::{Destination, Live};

use super::AppState;

impl AppState {
    /// Backs the finder out of full size or its filter while that page is in view (no page
    /// pushed over it); whether there was one to clear.
    pub(super) fn clear_page_selection(&self) -> bool {
        let (destination, bare) = self
            .nav
            .with_untracked(|nav| (nav.destination, nav.pages.is_empty()));
        if !bare {
            return false;
        }
        match destination {
            Destination::Artifacts => self.escape_artifacts(),
            _ => false,
        }
    }

    /// Escape on the session page on top: back to Chat and the composer from a view, else Stop
    /// while the agent works; whether it did either.
    pub(super) fn escape_session(&self) -> bool {
        let Some(Live::Session(id)) = self.nav.with_untracked(|nav| nav.live().cloned()) else {
            return false;
        };
        let env = env_of(self);
        let web = cfg!(target_arch = "wasm32");
        let action = self.feed.with_untracked(|feed| {
            self.session.with_untracked(|state| {
                let facts = Facts::of(feed, &id);
                let cx = Cx::new(feed, state.ui(id.as_str()), &id, env, &facts);
                session::press("escape", &cx, web)
            })
        });
        let Some(action) = action else {
            return false;
        };
        self.reduce(self.session, |state, feed, _, _| state.act(action, feed));
        true
    }
}

fn env_of(app: &AppState) -> crate::model::Env {
    nsh_core::prelude::untracked(|| app.env())
}
