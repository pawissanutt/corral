//! Key routing on desktop and in a terminal. The global keys (destinations, Ask, the palette, a
//! domain's letter commands) are bound for the app's life. A destination's list keys (moves,
//! Enter, digits) are bound only while its list has the keyboard focus, or nothing has: a
//! focused control keeps Tab, Enter, Space and the arrows, as the platform gives them to it.
//! While a session page is on top its views' bare keys (`session::keys`, those that are not
//! chords) come first, and only while no field is edited; a destination letter does not leave
//! the page. The domain reducers stay pure; this layer routes a key to the one serving it.

use std::collections::BTreeSet;

use nsh_core::keymap::When;
use nsh_core::prelude::*;

use super::AppState;
use crate::model::projects::{FilterScreen, NowScope, WorkspaceMode};
use crate::model::session::{self, Cx, Facts, Focus, KeyAction};
use crate::model::{DataState, Destination, Effect, Live, Overlay, commands, plans, review, work};

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

/// NSH names a shifted character by its unshifted key: `?` is Shift+/ and a reducer's capital
/// (`J`/`K`, Now's `R`, Review's `A`, a session view's `N`) is Shift+letter (a bare "J" would
/// bind `j`).
fn stroke(key: &str) -> String {
    match key {
        "?" => "shift+/".to_owned(),
        key if key.len() == 1 && key.bytes().all(|byte| byte.is_ascii_uppercase()) => {
            format!("shift+{}", key.to_ascii_lowercase())
        }
        key => key.to_owned(),
    }
}

/// The bare keys a session view declares (`j`, `r`, `enter`, `down`): its letters and its moves.
fn session_view_keys(form: crate::model::Form) -> impl Iterator<Item = &'static str> {
    let web = cfg!(target_arch = "wasm32");
    session::bound(form, web)
        .into_iter()
        .filter(|binding| binding.action == KeyAction::Part && !binding.typing())
        .map(|binding| binding.key)
}

/// One character (`j`, `N`, `?`): a key no focused control uses for itself.
fn is_letter(key: &str) -> bool {
    key.chars().count() == 1
}

impl AppState {
    /// The control `handle` takes the keyboard focus whenever a direct key names its test id
    /// (`Effect::Focus`), once it has laid out.
    pub fn focus_when_asked(self, id: &'static str, handle: ElementRef) {
        let focus_to = self.focus_to;
        effect(move || {
            if focus_to.get() == Some(id) {
                untracked(|| focus_to.set(None));
                after_answer(move || handle.focus());
            }
        });
    }

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
        let mut keys: BTreeSet<&'static str> = ["/", "?"].into_iter().collect();
        keys.extend(
            Destination::ALL
                .into_iter()
                .filter_map(commands::destination_key),
        );
        for domain in [work::KEYS, plans::KEYS, review::KEYS] {
            keys.extend(domain.iter().copied());
        }
        for &key in &keys {
            on_key_with(&stroke(key), typing, move |_| self.press_key(key));
        }
        // A session view's letters the global set does not bind already: they answer only while
        // a session page is on top, then fall back to the destination's list.
        let views: BTreeSet<&'static str> = session_view_keys(self.form)
            .filter(|key| is_letter(key) && !keys.contains(key))
            .collect();
        for &key in &views {
            on_key_with(&stroke(key), typing, move |_| {
                if !self.press_session(key) && untracked(|| self.list_keys_now().contains(&key)) {
                    self.press_key(key);
                }
            });
        }
        // Bound again whenever the focus or the page changes what the list keys reach; each run
        // drops the bindings of the one before.
        let list = computed(move || self.list_keys_now());
        let taken = keys.clone();
        let letters = views.clone();
        effect(move || {
            for &key in list.get() {
                // Destination/list collisions share one router, never two active bindings.
                if !taken.contains(key) && !letters.contains(key) {
                    on_key_with(&stroke(key), typing, move |_| self.press_key(key));
                }
            }
        });
        // A session view's moves and Enter, as a list's: only while the view has the keys and
        // no control has the focus, which keeps its own Enter and arrows.
        let moves = computed(move || self.session_moves_now());
        effect(move || {
            let listed = list.get();
            for key in moves.get() {
                if !keys.contains(key) && !views.contains(key) && !listed.contains(&key) {
                    on_key_with(&stroke(key), typing, move |_| {
                        self.press_session(key);
                    });
                }
            }
        });
        on_key_with("mod+k", anywhere, move |_| self.toggle_palette());
        on_key_with("escape", anywhere, move |_| self.close_newest());
        on_key_with("mod+b", typing, move |_| self.toggle_sidebar());
        // Save the configuration draft from any of its fields, as the save bar's Save does.
        on_key_with("mod+s", anywhere, move |_| {
            if self.nav.with_untracked(super::config::on_config) {
                self.act(self.settings, |state, _| state.save_config());
            }
        });
        // On a session page the session's model picker; elsewhere the next profile for Ask.
        on_key_with("mod+shift+m", anywhere, move |_| {
            if self.data.get_untracked() == DataState::Disconnected {
                return;
            }
            if self.press_session("mod+shift+m") {
                return;
            }
            let top = self.nav.with_untracked(|nav| nav.live().cloned());
            let ask_open = self.nav.with_untracked(|nav| nav.is_open(Overlay::Ask));
            self.reduce(self.assistant, |state, feed, _, env| {
                state.cycle_model_live(top.as_ref(), ask_open, feed, env)
            });
        });
    }

    /// Runs `key` on the session page on top, when one is and the key does something there
    /// now; whether it did. Nothing runs while a sheet, an overlay or a menu stands over it.
    fn press_session(&self, key: &str) -> bool {
        if untracked(|| self.sheet_open())
            || self.nav.with_untracked(|nav| nav.newest().is_some())
            || self.menus.get_untracked() > 0
        {
            return false;
        }
        let Some(Live::Session(id)) = self.nav.with_untracked(|nav| nav.live().cloned()) else {
            return false;
        };
        let env = untracked(|| self.env());
        let web = cfg!(target_arch = "wasm32");
        let action = self.feed.with_untracked(|feed| {
            self.session.with_untracked(|state| {
                let facts = Facts::of(feed, &id);
                let cx = Cx::new(feed, state.ui(id.as_str()), &id, env, &facts);
                session::press(key, &cx, web)
            })
        });
        let Some(action) = action else {
            return false;
        };
        self.reduce(self.session, |state, feed, _, _| state.act(action, feed));
        true
    }

    /// Whether a session page is on top.
    fn on_session(&self) -> bool {
        self.nav
            .with_untracked(|nav| matches!(nav.live(), Some(Live::Session(_))))
    }

    /// Tracked: the session view's moves and Enter to bind now: none unless a session page is
    /// on top with a view (not the composer) holding its keys, no control has the focus, and no
    /// sheet, overlay or menu stands over it.
    fn session_moves_now(&self) -> Vec<&'static str> {
        if !self.key_focus.get().takes_list_keys()
            || self.sheet_open()
            || self.nav.with(|nav| nav.newest().is_some())
            || self.menus.get() > 0
        {
            return Vec::new();
        }
        let Some(Live::Session(id)) = self.nav.with(|nav| nav.live().cloned()) else {
            return Vec::new();
        };
        if self
            .session
            .with(|state| state.ui(id.as_str()).focus == Focus::Composer)
        {
            return Vec::new();
        }
        let moves: BTreeSet<&'static str> = session_view_keys(self.form)
            .filter(|key| !is_letter(key))
            .collect();
        moves.into_iter().collect()
    }

    /// The phone form with a hardware keyboard: Ctrl+K (⌘K) opens and closes the palette, as the
    /// magnifier does. Its letter and list keys stay unbound; a phone has no key line naming them.
    pub(super) fn bind_phone_keys(self) {
        let anywhere = KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        };
        on_key_with("mod+k", anywhere, move |_| self.toggle_palette());
    }

    fn toggle_palette(self) {
        if self.is_open(Overlay::Palette) {
            self.close(Overlay::Palette);
        } else {
            self.open_palette();
        }
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

    /// Tracked: the list keys to bind now. None while a sheet, an overlay or a menu is up (its
    /// own items take the arrows and Enter: a menu the pointer opened takes Down into its first
    /// item), while a control outside the list has the focus, or without a connection.
    fn list_keys_now(&self) -> &'static [&'static str] {
        if !self.key_focus.get().takes_list_keys()
            || self.sheet_open()
            || self.nav.with(|nav| nav.newest().is_some())
            || self.work.with(|state| state.menu.is_some())
            || self.menus.get() > 0
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
            DataState::Daemon | DataState::Fixture => {
                let keys = self
                    .nav
                    .with(|nav| commands::list_keys_live(destination, nav.live()));
                let focus = self
                    .key_focus
                    .get()
                    .anywhere
                    .then(|| self.focused_row.get());
                // The selected row as the focused one is named: Now's task id, the roster's
                // `agent-<session>`.
                let selected = if destination == Destination::Agents {
                    self.agents.with(|state| {
                        state
                            .roster_session
                            .as_ref()
                            .map(|session| format!("agent-{session}"))
                    })
                } else {
                    self.nav
                        .with(|nav| nav.selection.task.as_ref().map(ToString::to_string))
                };
                commands::with_focus(
                    keys,
                    focus.as_ref().map(|row| row.as_deref()),
                    selected.as_deref(),
                )
            }
            DataState::Disconnected => &[],
        }
    }

    fn press_key(&self, key: &str) {
        if untracked(|| self.sheet_open())
            || self.nav.with_untracked(|nav| nav.newest().is_some())
            || self.work.with_untracked(|state| state.menu.is_some())
            || self.menus.get_untracked() > 0
        {
            return;
        }
        if self.press_session(key) {
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
            && !self.on_session()
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
                effects = state.key(key, top, feed, &filter);
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
    /// Tracked: the confirm sheet, the refusal sheet, the rewind sheet, the New build sheet, the
    /// Remove sheet or the action composer is up.
    pub(super) fn sheet_open(&self) -> bool {
        self.confirm.with(Option::is_some)
            || self.refusal.with(Option::is_some)
            || self.agents.with(|state| {
                state.rewind.is_some() || state.start.open || state.removing.is_some()
            })
            || self.work.with(|state| state.composer.is_some())
    }
}
