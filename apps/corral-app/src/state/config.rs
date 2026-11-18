//! Settings › Configuration's requests and its one question: `config.get` and `config.put` go
//! to corrald (or the sample preview's engine) under the editor's key, the answer settles the
//! editor, and leaving the section with unsaved edits asks once before they are dropped.

use nsh_core::prelude::*;

use super::AppState;
use crate::client::requests::{self, ClientRequest};
use crate::model::act::Confirming;
use crate::model::{Destination, Id, NavState, SettingsSection};

impl AppState {
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
                    request: corral_protocol::config::ConfigDocument { toml },
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
}

pub(super) fn on_config(nav: &NavState) -> bool {
    nav.destination == Destination::Settings && nav.settings == SettingsSection::Config
}
