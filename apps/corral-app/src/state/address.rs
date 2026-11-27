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
use crate::model::{Form, Live, TaskTab};

/// The place the page was opened at, read before the first screen is built so a reload never
/// shows another one first.
pub(super) fn launched() -> Option<Place> {
    #[cfg(target_arch = "wasm32")]
    {
        let url = web_sys::window()?.location().href().ok()?;
        Place::parse(url_path(&url))
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
                phone_section: self.phone_section.get(),
                phone_list: self.phone_list.get(),
            };
            let place = self.nav.with(|nav| Place::of(nav, form, shown));
            if matches!(place, Place::Task(_, TaskTab::Changes)) {
                self.impact.with(|state| place.with_impact(state))
            } else {
                place
            }
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
            // A former address names its new home: the entry takes the home's own path.
            let target = match target {
                Place::Project(id) => {
                    self.open_project(id);
                    place.get_untracked()
                }
                target => target,
            };
            // A former path (`/activity/system/runtime`) names a place whose own path differs:
            // the entry takes the place's own path, without a new entry.
            if url_path(&url) != href(&target) {
                let _ = set_app_url(href(&target));
            }
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
        match place {
            Place::Destination(destination) => self.go(destination),
            Place::List(area) if self.form == Form::Phone => self.phone_list_open.set(Some(area)),
            Place::List(area) => self.go_area(area),
            Place::Project(id) => self.open_project(id),
            Place::TaskImpact(ref id, _) | Place::Task(ref id, TaskTab::Changes) => {
                // The address must not see a Changes tab with its previous view or pick.
                batch(|| {
                    self.change(|nav| place.clone().open(nav));
                    if self.nav.with_untracked(|nav| {
                        nav.task_tab == TaskTab::Changes
                            && matches!(nav.live(), Some(Live::Task(open)) if open == id)
                    }) {
                        self.impact.update(|state| place.restore_impact(state));
                    }
                });
            }
            place => self.change(|nav| place.open(nav)),
        }
    }
}
