//! The browser's address bar. The page's path names the place the navigation shows, so a
//! reload or a link opens there, and each destination or page the owner visits on the desktop
//! is an entry the browser's Back and Forward return to. The phone's stack already makes its
//! pages the browser's entries, so there the address only follows (its shell tells which
//! Settings section is open). Native and terminal forms
//! have no address; they restore the stored destination.

use nsh_core::platform::Os;
use nsh_core::prelude::*;
use nsh_core::url_scheme::HistoryBehavior;

use super::AppState;
use crate::model::address::{Place, Shown, url_path};
use crate::model::{Form, Live};

/// The place the page was opened at, read before the first screen is built so a reload never
/// shows another one first.
pub(super) fn launched() -> Option<Place> {
    #[cfg(target_arch = "wasm32")]
    {
        let path = web_sys::window()?.location().pathname().ok()?;
        Place::parse(&path)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

fn href(place: &Place) -> String {
    format!("/{}", place.path())
}

impl AppState {
    /// Keeps the address on the place shown, and shows the place of every address the browser
    /// moves to itself: Back, Forward, or an entry an earlier load of the page left behind.
    pub(super) fn follow_address(self) {
        if try_os() != Some(Os::Web) {
            return;
        }
        let form = self.form;
        let place = computed(move || {
            let shown = Shown {
                inspector: self.work.with(|state| !state.digest_only),
                phone_section: self.phone_section.get(),
            };
            self.nav.with(|nav| Place::of(nav, form, shown))
        });
        // The place the address already names: what the page was opened at, the last one
        // written, or the one the browser moved to.
        let named = stored(None::<Place>);
        effect(move || {
            let now = place.get();
            let was = named.replace(Some(now.clone()));
            let history = match &was {
                Some(was) if *was == now => return,
                Some(was) if form == Form::Desktop && !was.same_page(&now) => HistoryBehavior::Push,
                _ => HistoryBehavior::Replace,
            };
            // A refused address only loses the place for the next reload.
            let _ = set_app_url_with(href(&now), history);
        });
        let follow = move |url: String| {
            let Some(target) = Place::parse(url_path(&url)) else {
                return;
            };
            // The entry has this address already: arriving there must not write another.
            named.replace(Some(target.clone()));
            if place.get_untracked() == target {
                return;
            }
            self.open_place(target.clone());
            // A move the app did not make (unsaved Configuration edits ask first, a workspace
            // without that destination): the entry takes the address of what still shows.
            let shown = place.get_untracked();
            if shown != target {
                let _ = set_app_url(href(&shown));
                named.replace(Some(shown));
            }
        };
        spawn(async move {
            loop {
                follow(next_app_url_change().await.url);
            }
        });
        spawn(async move {
            loop {
                follow(next_url_scheme_open().await);
            }
        });
    }

    /// Shows the place an address names, as opening it by name does.
    fn open_place(&self, place: Place) {
        match &place {
            Place::Destination(destination) => return self.go(*destination),
            Place::Live(live) => {
                self.reveal_live(live);
                if matches!(live, Live::Task(_)) {
                    self.work.update(|state| state.digest_only = false);
                }
            }
            Place::Settings(_) | Place::Project(_) => {}
        }
        let form = self.form;
        self.change(|nav| place.open(nav, form));
    }
}
