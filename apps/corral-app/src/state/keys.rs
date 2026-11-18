//! Key routing on desktop and in a terminal. The global keys (destinations, Ask, the palette, a
//! domain's letter commands) are bound for the app's life. A destination's list keys (moves,
//! Enter, digits) are bound only while its list has the keyboard focus, or nothing has: a
//! focused control keeps Tab, Enter, Space and the arrows, as the platform gives them to it.
//! The domain reducers stay pure; this layer routes a key to the one serving the destination.

use std::collections::BTreeSet;

use nsh_core::keymap::When;
use nsh_core::prelude::*;

use super::AppState;
use crate::model::projects::{FilterScreen, NowScope, WorkspaceMode};
use crate::model::{DataState, Destination, Effect, Overlay, commands, plans, review, work};

/// Where the keyboard focus is, as far as key routing needs it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct KeyFocus {
    /// A view or a field has the keyboard focus (within `AppState::focus_root`).
    anywhere: bool,
    /// The focus is within the destination's list (`AppState::list_keys`).
    list: bool,
}

impl KeyFocus {
    /// List keys act on the list it is in, or on the page while no control has the focus.
    fn takes_list_keys(self) -> bool {
        !self.anywhere || self.list
    }
}

/// NSH names a shifted character by its unshifted key: `?` is Shift+/ and the reducers'
/// capital `J`/`K` are Shift+J/K (a bare "J" would bind plain `j`).
fn stroke(key: &'static str) -> &'static str {
    match key {
        "?" => "shift+/",
        "J" => "shift+j",
        "K" => "shift+k",
        key => key,
    }
}

impl AppState {
    pub(super) fn bind_keys(self) {
        let typing = KeyOptions {
            when: When::NotEditing,
            ..KeyOptions::default()
        };
        let anywhere = KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        };
        self.watch_key_focus();
        let mut keys: BTreeSet<&'static str> = ["/", "?", "`"].into_iter().collect();
        keys.extend(
            Destination::ALL
                .into_iter()
                .filter_map(commands::destination_key),
        );
        for domain in [work::KEYS, plans::KEYS, review::KEYS] {
            keys.extend(domain.iter().copied());
        }
        for &key in &keys {
            on_key_with(stroke(key), typing, move |_| self.press_key(key));
        }
        // Bound again whenever the focus or the page changes what the list keys reach; each run
        // drops the bindings of the one before.
        let list = computed(move || self.list_keys_now());
        effect(move || {
            for &key in list.get() {
                // Destination/list collisions share one router, never two active bindings.
                if !keys.contains(key) {
                    on_key_with(stroke(key), typing, move |_| self.press_key(key));
                }
            }
        });
        on_key_with("mod+k", anywhere, move |_| {
            if self.is_open(Overlay::Palette) {
                self.close(Overlay::Palette);
            } else {
                self.open_palette();
            }
        });
        on_key_with("escape", anywhere, move |_| self.close_newest());
        on_key_with("mod+b", typing, move |_| self.toggle_sidebar());
        on_key_with("mod+shift+m", anywhere, move |_| {
            if self.data.get_untracked() == DataState::Disconnected {
                return;
            }
            let top = self.nav.with_untracked(|nav| nav.live().cloned());
            let ask_open = self.nav.with_untracked(|nav| nav.is_open(Overlay::Ask));
            self.reduce(self.assistant, |state, feed, _, env| {
                state.cycle_model_live(top.as_ref(), ask_open, feed, env)
            });
        });
    }

    /// Follows the keyboard focus coming into and leaving the app and the destination's list.
    fn watch_key_focus(self) {
        let focus = self.key_focus;
        self.focus_root
            .on_focus_in(move || focus.update(|at| at.anywhere = true));
        self.focus_root
            .on_focus_out(move || focus.update(|at| at.anywhere = false));
        self.list_keys
            .on_focus_in(move || focus.update(|at| at.list = true));
        self.list_keys
            .on_focus_out(move || focus.update(|at| at.list = false));
    }

    /// Tracked: the list keys to bind now. None while a sheet or an overlay is up (its own
    /// controls take Enter), while a control outside the list has the focus, or without a
    /// connection.
    fn list_keys_now(&self) -> &'static [&'static str] {
        if !self.key_focus.get().takes_list_keys()
            || self.sheet_open()
            || self.nav.with(|nav| nav.newest().is_some())
            || self.work.with(|state| state.menu.is_some())
        {
            return &[];
        }
        let destination = self.destination();
        if self
            .nav
            .with(|nav| matches!(nav.pages.last(), Some(crate::model::Route::Project(_))))
            || (self.workspace_mode() == WorkspaceMode::AssistantOnly
                && destination == Destination::Now)
        {
            return &[];
        }
        match self.data.get() {
            DataState::Daemon | DataState::Fixture => self
                .nav
                .with(|nav| commands::list_keys_live(destination, nav.live())),
            DataState::Disconnected => &[],
        }
    }

    fn press_key(&self, key: &str) {
        if untracked(|| self.sheet_open())
            || self.nav.with_untracked(|nav| nav.newest().is_some())
            || self.work.with_untracked(|state| state.menu.is_some())
        {
            return;
        }
        match key {
            "/" => {
                self.open_ask();
                return;
            }
            "?" => {
                self.open_palette();
                return;
            }
            "`" => {
                self.toggle_session();
                return;
            }
            _ => {}
        }
        // In Now/Review, '.' belongs to the focused list's selected-row menu. Otherwise it
        // remains the global Changes shortcut; an empty list still consumes its own key.
        let scoped_menu = key == "." && untracked(|| self.list_keys_now().contains(&"."));
        // Without a connection no destination is served, so only destination keys answer.
        let effects = if key == "." && !scoped_menu {
            None
        } else {
            match self.data.get_untracked() {
                DataState::Daemon | DataState::Fixture => self.key_live(key),
                DataState::Disconnected => None,
            }
        };
        if let Some(effects) = effects {
            self.run(effects);
        } else if !scoped_menu
            && let Some(destination) = Destination::ALL
                .into_iter()
                .find(|destination| commands::destination_key(*destination) == Some(key))
        {
            self.go(destination);
        }
    }

    /// Routes a key to the served destination's live reducer with the feed and selection.
    fn key_live(&self, key: &str) -> Option<Vec<Effect>> {
        let destination = self.nav.with_untracked(|nav| nav.destination);
        if self
            .nav
            .with_untracked(|nav| matches!(nav.pages.last(), Some(crate::model::Route::Project(_))))
            || (untracked(|| self.workspace_mode()) == WorkspaceMode::AssistantOnly
                && destination == Destination::Now)
        {
            return None;
        }
        if untracked(|| self.available(destination)) != crate::model::Availability::Ready {
            return None;
        }
        let (top, selection) = self
            .nav
            .with_untracked(|nav| (nav.live().cloned(), nav.selection.clone()));
        let top = top.as_ref();
        let env = self.env_untracked();
        let filter = untracked(|| {
            FilterScreen::of(destination)
                .map(|screen| self.project_filter(screen))
                .unwrap_or_default()
        });
        let grouping = untracked(|| self.now_group());
        let mut effects = None;
        self.feed.with_untracked(|feed| match destination {
            Destination::Now => self.work.update(|state| {
                effects = state.key_now_page(
                    key,
                    top,
                    &selection,
                    feed,
                    env,
                    NowScope {
                        filter: &filter,
                        grouping,
                    },
                );
            }),
            Destination::Plans => self.plans.update(|state| {
                effects = state.key_live_filtered(key, top, &selection, feed, env, &filter);
            }),
            Destination::Agents => self.agents.update(|state| {
                effects = state.key_live_filtered(key, top, &selection, feed, env, &filter);
            }),
            Destination::Review => self.review.update(|state| {
                effects = state.key_live_filtered(key, top, &selection, feed, env, &filter);
            }),
            _ => {}
        });
        effects
    }
}

impl AppState {
    /// Tracked: the confirm sheet, the refusal sheet or the action composer is up.
    pub(super) fn sheet_open(&self) -> bool {
        self.confirm.with(Option::is_some)
            || self.refusal.with(Option::is_some)
            || self.work.with(|state| state.composer.is_some())
    }
}
