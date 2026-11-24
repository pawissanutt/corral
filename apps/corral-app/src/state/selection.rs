//! Escape on a page with a selection: the selection closes before the page is left (the mock's
//! Esc), so the owner never loses the page they were reading to dismiss its side panel. On a
//! session page Escape first returns a view to Chat, then stops the working turn, and only then
//! leaves the page.

use crate::model::review::impact::View;
use crate::model::session::{self, Cx, Facts};
use crate::model::{Destination, Live, NavState, TaskTab};

use super::AppState;

impl AppState {
    /// Gives the visible selection (or map level) its Escape before leaving its page.
    pub(super) fn clear_page_selection(&self) -> bool {
        let (destination, bare) = self
            .nav
            .with_untracked(|nav| (nav.destination, nav.pages.is_empty()));
        if !bare {
            return self.escape_impact();
        }
        match destination {
            Destination::Artifacts => self.escape_artifacts(),
            Destination::System => self.escape_system(),
            _ => false,
        }
    }

    /// Tracked: what Escape does on the Impact view, if it stays on the task page.
    pub(crate) fn impact_escape_hint(&self) -> Option<&'static str> {
        self.nav.with(|nav| {
            let task = impact_task(nav)?;
            self.impact.with(|state| {
                (state.task.as_deref() == Some(task))
                    .then(|| state.escape_hint())
                    .flatten()
            })
        })
    }

    /// Whether a phone's hardware Escape should go through the Impact page's close rules.
    pub(super) fn on_impact_view(&self) -> bool {
        self.nav.with_untracked(|nav| impact_task(nav).is_some())
            && self
                .impact
                .with_untracked(|state| state.view == View::Impact)
    }

    fn escape_impact(&self) -> bool {
        if nsh_core::prelude::untracked(|| self.impact_escape_hint()).is_none() {
            return false;
        }
        let mut handled = false;
        self.impact.update(|state| handled = state.escape());
        handled
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

fn impact_task(nav: &NavState) -> Option<&str> {
    match nav.live() {
        Some(Live::Task(id) | Live::Plan(id) | Live::Review(id))
            if nav.task_tab == TaskTab::Changes =>
        {
            Some(id.as_str())
        }
        _ => None,
    }
}

fn env_of(app: &AppState) -> crate::model::Env {
    nsh_core::prelude::untracked(|| app.env())
}
