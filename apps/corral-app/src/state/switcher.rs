//! The Agents switcher's state: open or not, this device's pins and the conversation last
//! opened per authenticated enrollment and daemon store (`corral.agents.v2`; preview stays in memory, so no sample
//! id is ever saved), and New assistant, which keys Ask to the chat it starts once corrald
//! answers with it.

use nsh_core::{prelude::*, storage::Storage};

use super::AppState;
use crate::model::agents::{self, Pins, Switcher};
use crate::model::{DataState, Id, Live, Overlay, assistant};

// Store-only preferences cannot be attributed to an authenticated daemon safely.
const AGENTS_KEY: &str = "corral.agents.v2";

fn pin_key(feed: &crate::model::daemon::Feed, preview: bool) -> Option<String> {
    let store = feed.store.as_deref()?;
    let identity = if preview {
        "preview"
    } else {
        // The daemon issues this enrollment identity; a welcome alone is not enough.
        if !feed.ready() {
            return None;
        }
        feed.session.as_ref()?.device.id.as_str()
    };
    if identity.is_empty() || store.is_empty() {
        return None;
    }
    serde_json::to_string(&(identity, store)).ok()
}

pub(super) fn load_pins() -> Pins {
    Storage::get_item(AGENTS_KEY)
        .and_then(|value| Pins::parse(&value))
        .unwrap_or_default()
}

impl AppState {
    /// Tracked: the pins of the source in view: the saved ones, or the preview's own.
    fn pins_in_view(&self) -> Signal<Pins> {
        if self.data.get() == DataState::Fixture {
            self.preview_pins
        } else {
            self.pins
        }
    }

    /// Tracked: the switcher's list for the daemon store in view.
    pub fn agents_switcher(&self) -> Switcher {
        let pins = self.pins_in_view();
        let ask = self.ask_id();
        self.feed.with(|feed| {
            pins.with(|pins| {
                let saved = pin_key(feed, self.data.get() == DataState::Fixture)
                    .as_deref()
                    .and_then(|key| pins.get(key));
                agents::switcher(feed, saved, ask.as_str())
            })
        })
    }

    /// Tracked: whether the conversation is pinned for the store in view.
    pub fn agent_pinned(&self, id: &str) -> bool {
        let pins = self.pins_in_view();
        self.feed.with(|feed| {
            pin_key(feed, self.data.get() == DataState::Fixture)
                .as_deref()
                .is_some_and(|key| pins.with(|pins| pins.pinned(key, id)))
        })
    }

    /// Opens the switcher over whatever shows, the palette closed.
    pub fn open_agents(&self) {
        self.close(Overlay::Palette);
        self.switcher.set(true);
    }

    pub fn close_agents(&self) {
        self.switcher.set(false);
    }

    /// The chord's press: opens the switcher, or closes it while it shows.
    pub(super) fn toggle_agents(&self) {
        if self.switcher.get_untracked() {
            self.close_agents();
        } else {
            self.open_agents();
        }
    }

    /// Opens the conversation's page. A chat that has not ended becomes Ask's, so the next
    /// `/` goes on with it; a build only opens.
    pub fn switch_agent(&self, id: Id) {
        self.close_agents();
        let keys_ask = self.feed.with_untracked(|feed| {
            feed.assistants
                .get(id.as_str())
                .is_some_and(agents::switcher::keys_ask)
        });
        if keys_ask {
            let chat = id.clone();
            self.session.update(|state| state.ask = Some(chat));
        }
        self.open_live(Live::Session(id));
    }

    /// Pins or unpins the exact conversation only for a ready authenticated store.
    pub fn toggle_agent_pin(&self, id: &Id) {
        let preview = self.data.get_untracked() == DataState::Fixture;
        let Some(store) = self.feed.with_untracked(|feed| pin_key(feed, preview)) else {
            self.toast("Not pinned: no authenticated daemon store is ready.");
            return;
        };
        untracked(|| self.pins_in_view()).update(|pins| _ = pins.toggle(&store, id.as_str()));
    }

    /// Starts a new chat apart from the one Ask is on, which stays as it is: once corrald
    /// answers with the chat, Ask is keyed to it and its page opens.
    pub fn new_assistant(&self) {
        self.close_agents();
        self.reduce(self.assistant, |state, feed, _, _| {
            state.act_live(assistant::Action::NewSession, feed)
        });
        let key = self
            .assistant
            .with_untracked(|state| state.live.new_session.clone());
        self.switch_new.set(key);
    }

    /// Persists the saved pins; remembers each assistant conversation whose page opens as the
    /// one last opened; keys Ask to the chat New assistant started; and settles the chat
    /// operations wherever they were sent from, so a new chat opens from any page.
    pub(super) fn follow_agents(self) {
        // The open picker snapshots its rows for keyboard walking. Dismiss that snapshot
        // rather than acting on A-1 from a different enrollment after a reconnect.
        let source = computed(move || {
            let preview = self.data.get() == DataState::Fixture;
            self.feed.with(|feed| pin_key(feed, preview))
        });
        effect(move || {
            let _ = source.get();
            self.close_agents();
        });
        effect(move || {
            if let Ok(encoded) = self.pins.with(Pins::encode) {
                let _ = Storage::set_item(AGENTS_KEY, &encoded);
            }
        });
        let opened = computed(move || {
            let Some(Live::Session(id)) = self.live_page() else {
                return None;
            };
            let preview = self.data.get() == DataState::Fixture;
            self.feed.with(|feed| {
                let exact = agents::exact(feed, id.as_str());
                Some((id.to_string(), pin_key(feed, preview), exact, preview))
            })
        });
        // An already-open page can survive a source change. Its id does not mean that the
        // user opened the same-named conversation on the newly authenticated daemon.
        let last_page = signal(None::<String>);
        effect(move || {
            let current = opened.get();
            untracked(|| {
                let previous = last_page.get_untracked();
                last_page.set(current.as_ref().map(|(page, _, _, _)| page.clone()));
                if let Some((page, Some(store), Some(id), preview)) = current {
                    if previous.as_ref() == Some(&page) {
                        return;
                    }
                    let pins = if preview { self.preview_pins } else { self.pins };
                    if pins.with_untracked(|pins| pins.get(&store)?.last.clone())
                        != Some(id.clone())
                    {
                        pins.update(|pins| _ = pins.used(&store, &id));
                    }
                }
            });
        });
        effect(move || {
            let Some(key) = self.switch_new.get() else {
                return;
            };
            let reply = self.with_operation(&key, |op| {
                op.filter(|op| !op.loading)
                    .map(|op| op.decode::<corral_protocol::rows::AssistantRow>())
            });
            let created = match reply {
                None if self
                    .assistant
                    .with(|state| state.live.new_session.is_some()) =>
                {
                    return;
                }
                None => None,
                Some(row) => row,
            };
            untracked(|| {
                self.switch_new.set(None);
                if let Some(row) = created {
                    self.session
                        .update(|state| state.ask = Some(Id::from(row.id.as_str())));
                }
            });
        });
        crate::screens::assistant::watch_ops(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::daemon::{Feed, Link};
    use corral_protocol::{
        app::{AppSession, SeenCursor},
        device::{DeviceInfo, DevicePlatform},
    };

    fn ready(device: &str, store: &str) -> Feed {
        Feed {
            link: Link::Up,
            transport_ready: true,
            projection_ready: true,
            store: Some(store.into()),
            session: Some(AppSession {
                device: DeviceInfo {
                    id: device.into(),
                    name: String::new(),
                    platform: DevicePlatform::Native,
                    app_version: String::new(),
                    enrolled_at: String::new(),
                    revoked_at: None,
                    last_seen: None,
                    reach: None,
                },
                seen: SeenCursor::default(),
                server_time: String::new(),
                local: Some(true),
            }),
            ..Feed::default()
        }
    }

    #[test]
    fn pins_do_not_cross_daemons_or_transition_through_unauthenticated_feed() {
        let a = ready("dev-a", "/same");
        let b = ready("dev-b", "/same");
        let key_a = pin_key(&a, false).unwrap();
        let key_b = pin_key(&b, false).unwrap();
        let mut pins = Pins::default();
        pins.toggle(&key_a, "A-1");
        pins.used(&key_a, "A-1");
        assert!(pins.get(&key_b).is_none());
        let mut transitioning = a.clone();
        transitioning.transport_ready = false;
        assert!(pin_key(&transitioning, false).is_none());
        transitioning.session = None;
        transitioning.transport_ready = true;
        assert!(pin_key(&transitioning, false).is_none());
        assert_ne!(pin_key(&ready("dev-a", "/other"), false), Some(key_a));
        assert_ne!(pin_key(&ready("dev-a", "/same"), true), Some(key_b));
    }
}
