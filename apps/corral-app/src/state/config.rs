//! Settings › Configuration's requests and its one question: `config.get` and `config.put` go
//! to corrald (or the sample preview's engine) under the editor's key, the answer settles the
//! editor, and leaving the section with unsaved edits asks once before they are dropped.

use nsh_core::prelude::*;

use super::AppState;
use crate::client::requests::{self, ClientRequest};
use crate::model::act::Confirming;
use crate::model::settings::editor::{Editor, Part};
use crate::model::settings::{automation, document};
use crate::model::{Destination, Id, NavState, SettingsSection};

impl AppState {
    pub fn automation_preview(&self) -> bool {
        self.fixture()
    }

    pub fn automations_available(&self) -> bool {
        self.feed
            .with(|feed| automation::available(feed, self.automation_preview()))
    }

    pub fn save_config(&self) {
        let available = untracked(|| self.automations_available());
        self.act(self.settings, move |state, _| {
            if state.config.waiting.is_some() {
                return Vec::new();
            }
            if automation::save_blocked(
                available,
                state.config.saved.as_deref(),
                &state.config.draft,
            ) {
                return state.config_unsent(automation::UNAVAILABLE.into());
            }
            state.save_config()
        });
    }

    /// Sends `config.get`, or `config.put` with `put`, under `key`; the sample preview answers
    /// it locally. One that cannot leave settles the editor with why.
    pub(super) fn send_config(&self, key: Id, put: Option<String>) {
        let unsent = if self.fixture() {
            self.feed
                .update(|feed| crate::model::preview::config(feed, &key, put.as_deref()));
            None
        } else if let Some(refusal) = self.link_refusal() {
            Some(refusal.to_owned())
        } else {
            let request_id = key.to_string();
            let sent = requests::send(match put {
                Some(toml) => ClientRequest::ConfigPut {
                    request_id,
                    request: corral_protocol::config::ConfigDocument {
                        toml,
                        revision: None,
                    },
                },
                None => ClientRequest::ConfigGet { request_id },
            });
            (!sent).then(|| "Not sent: the app's connection task is not running.".to_owned())
        };
        if let Some(why) = unsent {
            self.act(self.settings, move |state, _| state.config_unsent(why));
        }
    }

    /// Settles the editor once corrald answers the request it waits on.
    pub(super) fn settle_config(self) {
        effect(move || {
            let Some(key) = self.settings.with(|state| {
                state
                    .config
                    .waiting
                    .as_ref()
                    .map(|waiting| waiting.key.clone())
            }) else {
                return;
            };
            let answered = self.feed.with(|feed| {
                feed.operations
                    .get(&key)
                    .is_some_and(|operation| !operation.loading)
            });
            if answered {
                untracked(|| {
                    self.reduce(self.settings, |state, feed, _, _| state.settle_config(feed));
                });
            }
        });
    }

    /// Whether `next` leaves Configuration, where the owner is, while it holds unsaved edits;
    /// if so the confirm sheet asks first and holds where the owner was going.
    pub(super) fn asks_to_leave(&self, next: &NavState) -> bool {
        if on_config(next) || !self.settings.with_untracked(|state| state.config.dirty()) {
            return false;
        }
        self.refusal.set(None);
        self.confirm
            .set(Some(Confirming::Leave(Box::new(next.clone()))));
        true
    }

    /// The phone's back from its pushed Configuration page, whose leaving the model's
    /// navigation does not see: with unsaved edits the confirm sheet asks first, and its
    /// Discard lands on the Settings list (a `go`, which the phone's stack follows).
    pub fn asks_before_leaving_config(&self) -> bool {
        if !self.settings.with_untracked(|state| state.config.dirty()) {
            return false;
        }
        let mut next = self.nav.get_untracked();
        next.go(Destination::Settings);
        self.refusal.set(None);
        self.confirm.set(Some(Confirming::Leave(Box::new(next))));
        true
    }

    /// Tracked: whether one profile shows alone in the Profiles part, in its list's place (on
    /// the phone, and wherever the list has no room beside it): Back returns to the list.
    pub fn profile_alone(&self) -> bool {
        self.settings.with(|state| alone(&state.config))
    }

    /// Back from a profile shown alone: the list shows again and the draft stays as it is;
    /// whether it did.
    pub fn back_to_profiles(&self) -> bool {
        if !untracked(|| self.profile_alone()) {
            return false;
        }
        self.act(self.settings, |state, _| {
            state.config.profile_focus = false;
            Vec::new()
        });
        true
    }

    /// Escape on Configuration with a profile shown alone, once nothing stands over the page (a
    /// sheet, an overlay, a menu, the sidebar's labels): back to the list; whether it did.
    pub(super) fn escape_profile(&self) -> bool {
        let covered = untracked(|| self.sheet_open() || self.dock_overlays_page())
            || self.nav.with_untracked(|nav| nav.newest().is_some())
            || self.work.with_untracked(|state| state.menu.is_some())
            || self.menus.get_untracked() > 0
            || self.sidebar_peek.get_untracked();
        !covered
            && self
                .nav
                .with_untracked(|nav| nav.pages.is_empty() && on_config(nav))
            && self.back_to_profiles()
    }
}

pub(super) fn on_config(nav: &NavState) -> bool {
    nav.destination == Destination::Settings && nav.settings == SettingsSection::Config
}

/// The Profiles part shows the chosen profile without its list: it is open, the screen has no
/// room for the list beside it, and the draft has a profile to show.
fn alone(editor: &Editor) -> bool {
    editor.part == Part::Profiles
        && editor.profile_focus
        && !editor.profile_beside
        && document::parse(&editor.draft)
            .is_ok_and(|doc| !document::names(&doc, "profiles").is_empty())
}
