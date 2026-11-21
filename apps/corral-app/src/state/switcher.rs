//! The Agents switcher's state: open or not, this device's pins and the conversation last
//! opened per daemon store (`corral.agents`; the explicit preview's stay in memory, so no sample
//! id is ever saved), and New assistant, which keys Ask to the chat it starts once corrald
//! answers with it.

use nsh_core::{prelude::*, storage::Storage};

use super::AppState;
use crate::model::agents::{self, Pins, Switcher};
use crate::model::{DataState, Id, Live, Overlay, assistant};

const AGENTS_KEY: &str = "corral.agents";

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
                let saved = feed.store.as_deref().and_then(|store| pins.get(store));
                agents::switcher(feed, saved, ask.as_str())
            })
        })
    }

    /// Tracked: whether the conversation is pinned for the store in view.
    pub fn agent_pinned(&self, id: &str) -> bool {
        let pins = self.pins_in_view();
        self.feed.with(|feed| {
            feed.store
                .as_deref()
                .is_some_and(|store| pins.with(|pins| pins.pinned(store, id)))
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

    /// Pins or unpins the exact conversation for the store in view; nothing before the daemon
    /// names its store.
    pub fn toggle_agent_pin(&self, id: &Id) {
        let Some(store) = self.feed.with_untracked(|feed| feed.store.clone()) else {
            self.toast("Not pinned: corrald has not named its store yet.");
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
        effect(move || {
            if let Ok(encoded) = self.pins.with(Pins::encode) {
                let _ = Storage::set_item(AGENTS_KEY, &encoded);
            }
        });
        let opened = computed(move || {
            let Some(Live::Session(id)) = self.live_page() else {
                return None;
            };
            self.feed.with(|feed| {
                let exact = agents::exact(feed, id.as_str())?;
                Some((feed.store.clone()?, exact))
            })
        });
        effect(move || {
            if let Some((store, id)) = opened.get() {
                untracked(|| {
                    let pins = self.pins_in_view();
                    if pins.with_untracked(|pins| pins.get(&store)?.last.clone())
                        != Some(id.clone())
                    {
                        pins.update(|pins| _ = pins.used(&store, &id));
                    }
                });
            }
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
