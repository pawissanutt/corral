//! The app's signals over the model: where the user is, what they chose and what they typed, kept in
//! step with Storage and driven by the global keys.

use nsh_core::keymap::When;
use nsh_core::prelude::*;
use nsh_core::storage::Storage;

use crate::model::{
    AskTarget, DataState, Destination, Form, NavState, Overlay, SettingsSection, Snapshot,
    ThemeChoice,
};

const DESTINATION_KEY: &str = "corral.destination";
const SETTINGS_KEY: &str = "corral.settings";
const THEME_KEY: &str = "corral.theme";
const DATA_KEY: &str = "corral.data";
const NOW_LAYOUT_KEY: &str = "corral.now.layout";

#[derive(Clone, Copy)]
pub struct AppState {
    pub form: Form,
    pub nav: Signal<NavState>,
    pub data: Signal<DataState>,
    pub theme: Signal<ThemeChoice>,
    pub ask_target: Signal<AskTarget>,
    pub ask_draft: Signal<String>,
    /// Grows by one each time Ask is opened; the composer focuses its field when it changes.
    pub ask_focus: Signal<u32>,
    /// Grows by one when the Now dock layout must return to its default.
    pub now_reset: Signal<u32>,
}

impl AppState {
    /// Creates the state from Storage, keeps Storage in step with it, applies the theme choice and
    /// binds the global keys. Called once, in `App`.
    pub fn install(form: Form) -> AppState {
        let mut nav = NavState::new(form);
        if let Some(destination) = stored(DESTINATION_KEY, Destination::from_slug) {
            nav.destination = destination;
        }
        if let Some(section) = stored(SETTINGS_KEY, SettingsSection::from_slug) {
            nav.settings = section;
        }
        let app = AppState {
            form,
            nav: signal(nav),
            data: signal(stored(DATA_KEY, DataState::from_slug).unwrap_or(DataState::Fixture)),
            theme: signal(stored(THEME_KEY, ThemeChoice::from_slug).unwrap_or(ThemeChoice::System)),
            ask_target: signal(AskTarget::Assistant),
            ask_draft: signal(String::new()),
            ask_focus: signal(0),
            now_reset: signal(0),
        };
        app.persist();
        if form != Form::Phone {
            app.bind_keys();
        }
        app
    }

    /// Tracked: reads `data`.
    pub fn snapshot(&self) -> Snapshot {
        crate::model::snapshot(self.data.get())
    }

    /// Tracked.
    pub fn destination(&self) -> Destination {
        self.nav.with(|nav| nav.destination)
    }

    /// Tracked.
    pub fn is_open(&self, overlay: Overlay) -> bool {
        self.nav.with(|nav| nav.is_open(overlay))
    }

    pub fn go(&self, destination: Destination) {
        self.change(|nav| nav.go(destination));
    }

    pub fn go_settings(&self, section: SettingsSection) {
        self.change(|nav| nav.go_settings(section));
    }

    /// Opens Ask and bumps `ask_focus`.
    pub fn open_ask(&self) {
        self.change(|nav| nav.open(Overlay::Ask));
        self.focus_ask();
    }

    /// Sets the draft, then `open_ask`.
    pub fn open_ask_with(&self, text: &str) {
        self.ask_draft.set(text.to_owned());
        self.open_ask();
    }

    pub fn open_palette(&self) {
        self.change(|nav| nav.open(Overlay::Palette));
    }

    pub fn close(&self, overlay: Overlay) {
        self.change(|nav| nav.close(overlay));
    }

    pub fn close_newest(&self) {
        self.change(|nav| {
            nav.close_newest();
        });
    }

    /// The effect `install` starts on `theme` calls `set_color_scheme` with the new choice.
    pub fn set_theme(&self, choice: ThemeChoice) {
        self.theme.set(choice);
    }

    pub fn set_data(&self, state: DataState) {
        self.data.set(state);
    }

    /// Removes the stored layout and bumps `now_reset`.
    pub fn reset_now_layout(&self) {
        Storage::remove_item(NOW_LAYOUT_KEY);
        self.now_reset.update(|n| *n = n.wrapping_add(1));
    }

    /// Writes the navigation back only when `f` changed it, so readers of `nav` do not run again for
    /// a no-op such as Esc with nothing open.
    fn change(&self, f: impl FnOnce(&mut NavState)) {
        let mut nav = self.nav.get_untracked();
        f(&mut nav);
        self.nav.set(nav);
    }

    fn focus_ask(&self) {
        self.ask_focus.update(|n| *n = n.wrapping_add(1));
    }

    /// Each preference is written when it changes; memos keep overlay changes from rewriting the
    /// destination and section.
    fn persist(self) {
        let nav = self.nav;
        let destination = computed(move || nav.with(|nav| nav.destination));
        effect(move || remember(DESTINATION_KEY, destination.get().slug()));
        let settings = computed(move || nav.with(|nav| nav.settings));
        effect(move || remember(SETTINGS_KEY, settings.get().slug()));
        let theme = self.theme;
        effect(move || {
            let choice = theme.get();
            set_color_scheme(appearance(choice));
            remember(THEME_KEY, choice.slug());
        });
        let data = self.data;
        effect(move || remember(DATA_KEY, data.get().slug()));
    }

    /// Single-character keys never fire while a text field has the focus (`When::NotEditing`), so
    /// typing in Ask or the palette cannot navigate. `mod+k` and `escape` fire everywhere
    /// (`When::Always`): with `When::Auto` a focused field would swallow `escape`, which counts as
    /// a key that edits text.
    fn bind_keys(self) {
        let typing = KeyOptions {
            when: When::NotEditing,
            ..KeyOptions::default()
        };
        let anywhere = KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        };
        for key in Destination::ALL
            .map(Destination::key)
            .into_iter()
            .chain(['/'])
        {
            on_key_with(&key.to_string(), typing, move |_| self.press_key(key));
        }
        on_key_with("mod+k", anywhere, move |_| self.open_palette());
        on_key_with("escape", anywhere, move |_| self.close_newest());
    }

    fn press_key(self, key: char) {
        let mut nav = self.nav.get_untracked();
        let ask_was_open = nav.is_open(Overlay::Ask);
        if !nav.press_key(key) {
            return;
        }
        let ask_opened = !ask_was_open && nav.is_open(Overlay::Ask);
        self.nav.set(nav);
        if ask_opened {
            self.focus_ask();
        }
    }
}

/// The stored value of `key`, when there is one and it parses.
fn stored<T>(key: &str, parse: fn(&str) -> Option<T>) -> Option<T> {
    Storage::get_item(key).and_then(|value| parse(&value))
}

/// Storage refuses a write only when it is full. The crate has no logging dependency (nsh-core's
/// prelude re-exports none), and a lost write only means the next start opens on the default, so
/// the refusal is dropped.
fn remember(key: &str, slug: &str) {
    let _ = Storage::set_item(key, slug);
}

fn appearance(choice: ThemeChoice) -> Appearance {
    match choice {
        ThemeChoice::System => Appearance::System,
        ThemeChoice::Light => Appearance::Light,
        ThemeChoice::Dark => Appearance::Dark,
    }
}
