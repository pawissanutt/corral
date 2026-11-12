//! App signals, persistence and key routing. Domain reducers stay pure; this layer applies their
//! effects in order and owns transient confirmations, overlays and toast timing.

use std::collections::BTreeSet;
use std::time::Duration;

use nsh_core::keymap::When;
use nsh_core::prelude::*;
use nsh_core::storage::Storage;

use crate::model::commands::Action;
use crate::model::{
    AskTarget, Conn, DataMode, DataState, Destination, Effect, Env, Form, NavState, Overlay, Page,
    Risk, SettingsSection, Snapshot, ThemeChoice, agents, assistant, commands, inbox, plans,
    review, settings, timeline, usage, work,
};

const DESTINATION_KEY: &str = "corral.destination";
const SETTINGS_KEY: &str = "corral.settings";
const THEME_KEY: &str = "corral.theme";
const DATA_KEY: &str = "corral.data";
const CONN_KEY: &str = "corral.conn";
const MODE_KEY: &str = "corral.mode";
const NOW_LAYOUT_KEY: &str = "corral.now.layout";

/// Shared app handles. Screens render tracked reads and send domain effects back through `run`.
#[derive(Clone, Copy)]
pub struct AppState {
    pub form: Form,
    pub nav: Signal<NavState>,
    pub data: Signal<DataState>,
    pub theme: Signal<ThemeChoice>,
    pub ask_target: Signal<AskTarget>,
    pub ask_draft: Signal<String>,
    /// Grows by one each time Ask opens, so the composer focuses its field.
    pub ask_focus: Signal<u32>,
    /// Grows by one when the Now dock layout returns to its default.
    pub now_reset: Signal<u32>,
    /// Persisted connection demo switch, independent of fixture transport.
    pub conn: Signal<Conn>,
    /// Persisted loading, empty, error and answered-elsewhere switch.
    pub mode: Signal<DataMode>,
    pub inbox: Signal<inbox::State>,
    pub work: Signal<work::State>,
    pub plans: Signal<plans::State>,
    pub timeline: Signal<timeline::State>,
    pub usage: Signal<usage::State>,
    pub agents: Signal<agents::State>,
    pub review: Signal<review::State>,
    pub settings: Signal<settings::State>,
    pub assistant: Signal<assistant::State>,
    /// Fresh confirmation, with an optional device or idea name.
    pub confirm: Signal<Option<(Risk, Option<String>)>>,
    /// A receipt that expires 3.4 seconds after the most recent toast call.
    pub toast: Signal<Option<String>>,
    /// Later UI phases render the global hotkey window when true.
    pub hotkey: Signal<bool>,
    /// Later UI phases render the share-sheet demo when true.
    pub share: Signal<bool>,
    /// Later UI phases render the lock-screen push demo when true.
    pub lock: Signal<bool>,
}

impl AppState {
    /// Restores preferences, starts root-owned persistence and toast timing, and binds non-phone keys.
    pub fn install(form: Form) -> AppState {
        let mut nav = NavState::new(form);
        if let Some(destination) = preference(DESTINATION_KEY, Destination::from_slug) {
            nav.destination = destination;
        }
        if let Some(section) = preference(SETTINGS_KEY, SettingsSection::from_slug) {
            nav.settings = section;
        }
        let app = AppState {
            form,
            nav: signal(nav),
            data: signal(preference(DATA_KEY, DataState::from_slug).unwrap_or(DataState::Fixture)),
            theme: signal(
                preference(THEME_KEY, ThemeChoice::from_slug).unwrap_or(ThemeChoice::System),
            ),
            ask_target: signal(AskTarget::Assistant),
            ask_draft: signal(String::new()),
            ask_focus: signal(0),
            now_reset: signal(0),
            conn: signal(preference(CONN_KEY, Conn::from_slug).unwrap_or_default()),
            mode: signal(preference(MODE_KEY, DataMode::from_slug).unwrap_or_default()),
            inbox: signal(inbox::State::default()),
            work: signal(work::State::default()),
            plans: signal(plans::State::default()),
            timeline: signal(timeline::State::default()),
            usage: signal(usage::State::default()),
            agents: signal(agents::State::default()),
            review: signal(review::State::default()),
            settings: signal(settings::State::default()),
            assistant: signal(assistant::State::default()),
            confirm: signal(None),
            toast: signal_with(None, SignalEquals::Never),
            hotkey: signal(false),
            share: signal(false),
            lock: signal(false),
        };
        app.persist();
        app.install_toast_timer();
        if form != Form::Phone {
            app.bind_keys();
        }
        app
    }

    /// Tracked environment for any domain's reducer or view builder.
    pub fn env(&self) -> Env {
        Env {
            conn: self.conn.get(),
            mode: self.mode.get(),
            form: self.form,
        }
    }

    /// Tracked shell snapshot, with the current Inbox reducer's needs-you count.
    pub fn snapshot(&self) -> Snapshot {
        let env = self.env();
        let mut snapshot = crate::model::snapshot(self.data.get(), env.conn, self.form);
        if snapshot.connected() {
            snapshot.set_count(
                Destination::Inbox,
                self.inbox.with(|state| inbox::needs_you_count(state, env)),
            );
        }
        snapshot
    }

    /// Tracked current destination, including the owner of the top page.
    pub fn destination(&self) -> Destination {
        self.nav.with(|nav| nav.destination)
    }

    /// Tracked top page, absent when a task or plan is selected in place.
    pub fn page(&self) -> Option<Page> {
        self.nav.with(NavState::page)
    }

    /// Tracked overlay visibility.
    pub fn is_open(&self, overlay: Overlay) -> bool {
        self.nav.with(|nav| nav.is_open(overlay))
    }

    /// Changes destination and clears all pushed pages.
    pub fn go(&self, destination: Destination) {
        self.change(|nav| nav.go(destination));
    }

    /// Opens a Settings section and clears all pushed pages.
    pub fn go_settings(&self, section: SettingsSection) {
        self.change(|nav| nav.go_settings(section));
    }

    /// Updates the owning domain before selecting or pushing its page.
    pub fn open(&self, page: Page) {
        match page {
            Page::Task(id) => self.work.update(|state| state.select(id)),
            Page::Plan(id) => self.plans.update(|state| state.open(id)),
            Page::Session(id) => self.agents.update(|state| state.open(id)),
            Page::Thread(id) => {
                self.assistant.update(|state| state.open_thread(id));
                self.close(Overlay::Ask);
            }
            Page::Question => {}
        }
        self.change(|nav| nav.open_page(page, self.form));
    }

    /// Returns to the page beneath this one, or its owning destination.
    pub fn back(&self) {
        self.change(|nav| {
            nav.back();
        });
    }

    /// Opens Ask and requests composer focus without discarding its current context.
    pub fn open_ask(&self) {
        self.change(|nav| nav.open(Overlay::Ask));
        self.focus_ask();
    }

    /// Sets the draft, then opens Ask with the field focused.
    pub fn open_ask_with(&self, text: &str) {
        self.ask_draft.set(text.to_owned());
        self.open_ask();
    }

    /// Attaches the optional context chip before opening Ask.
    pub fn open_ask_about(&self, chip: Option<&'static str>) {
        self.assistant.update(|state| state.attach(chip));
        self.open_ask();
    }

    /// Opens or raises the palette above other overlays.
    pub fn open_palette(&self) {
        self.change(|nav| nav.open(Overlay::Palette));
    }

    /// Closes an overlay; leaving Ask also detaches its contextual chip.
    pub fn close(&self, overlay: Overlay) {
        self.change(|nav| nav.close(overlay));
        if overlay == Overlay::Ask {
            self.assistant.update(|state| state.attach(None));
        }
    }

    /// Escape priority: confirmation, model list, newest overlay, then top page.
    pub fn close_newest(&self) {
        if self.confirm.get_untracked().is_some() {
            self.confirm_no();
            return;
        }
        let mut closed = false;
        self.assistant.update(|state| closed = state.close_menu());
        if closed {
            return;
        }
        if let Some(overlay) = self.nav.with_untracked(NavState::newest) {
            self.close(overlay);
            return;
        }
        self.back();
    }

    /// Changes the persisted appearance and system color scheme.
    pub fn set_theme(&self, choice: ThemeChoice) {
        self.theme.set(choice);
    }

    /// Changes the existing fixture transport switch.
    pub fn set_data(&self, state: DataState) {
        self.data.set(state);
    }

    /// Removes the stored layout and asks Now to restore its default arrangement.
    pub fn reset_now_layout(&self) {
        Storage::remove_item(NOW_LAYOUT_KEY);
        self.now_reset.update(|n| *n = n.wrapping_add(1));
    }

    /// Shows a receipt; repeating the same text also restarts its root-owned delay.
    pub fn toast(&self, text: impl Into<String>) {
        self.toast.set(Some(text.into()));
    }

    /// Opens fresh confirmation, refusing risky commands while the daemon is unreachable.
    pub fn confirm(&self, risk: Risk, name: Option<String>) {
        if self.conn.get_untracked().refuses() {
            self.toast("Refused: needs a live connection. Nothing was queued.");
            return;
        }
        self.confirm.set(Some((risk, name)));
    }

    /// Closes confirmation and shows its named, device-attributed success receipt.
    pub fn confirm_yes(&self) {
        let Some((risk, name)) = self.confirm.get_untracked() else {
            return;
        };
        self.confirm.set(None);
        let name = name
            .filter(|name| !name.is_empty())
            .map(|name| format!(" · {name}"))
            .unwrap_or_default();
        self.toast(format!(
            "{}{name} · actor=owner via=app:{}",
            commands::risk(risk).ok,
            self.env().device()
        ));
    }

    /// Cancels the preview without applying any effect.
    pub fn confirm_no(&self) {
        self.confirm.set(None);
    }

    /// Applies every reducer effect in source order.
    pub fn run(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Toast(text) => self.toast(text),
                Effect::Go(destination) => self.go(destination),
                Effect::GoSettings(section) => self.go_settings(section),
                Effect::Open(page) => self.open(page),
                Effect::Back => self.back(),
                Effect::Confirm(risk, name) => self.confirm(risk, name),
                Effect::Ask { chip, text } => {
                    self.assistant.update(|state| state.attach(chip));
                    if let Some(text) = text {
                        self.ask_draft.set(text);
                    }
                    self.open_ask();
                }
                Effect::CloseAsk => self.close(Overlay::Ask),
                Effect::SetConn(conn) => self.conn.set(conn),
            }
        }
    }

    /// Runs a domain reducer on its state, then the effects it returns: what every button,
    /// row and option of a screen calls.
    pub fn act<S: Clone + 'static>(
        &self,
        state: Signal<S>,
        reduce: impl FnOnce(&mut S, Env) -> Vec<Effect>,
    ) {
        let env = Env {
            conn: self.conn.get_untracked(),
            mode: self.mode.get_untracked(),
            form: self.form,
        };
        let mut effects = Vec::new();
        state.update(|state| effects = reduce(state, env));
        self.run(effects);
    }

    /// Executes a model palette action after closing the palette.
    pub fn run_command(&self, action: Action) {
        self.close(Overlay::Palette);
        match action {
            Action::Go(destination) => self.go(destination),
            Action::Settings(section) => self.go_settings(section),
            Action::Open(page) => self.open(page),
            Action::Ask(text) => match text {
                Some(text) => self.open_ask_with(&text),
                None => self.open_ask(),
            },
            Action::ToggleSession => self.toggle_session(),
            Action::Theme(theme) => self.set_theme(theme),
            Action::Data(data) => self.set_data(data),
            Action::ResetNow => self.reset_now_layout(),
            Action::Connection(conn) => self.conn.set(conn),
            Action::Mode(mode) => self.mode.set(mode),
            Action::Assistant(mode) => self.assistant.update(|state| state.set_mode(mode)),
            Action::Hotkey => self.hotkey.set(true),
            Action::Share => {
                self.lock.set(false);
                self.close(Overlay::Ask);
                self.share.set(true);
            }
            Action::Lock => {
                self.share.set(false);
                self.lock.set(true);
            }
        }
    }

    fn change(&self, f: impl FnOnce(&mut NavState)) {
        let mut nav = self.nav.get_untracked();
        f(&mut nav);
        self.nav.set(nav);
    }

    fn focus_ask(&self) {
        self.ask_focus.update(|n| *n = n.wrapping_add(1));
    }

    fn install_toast_timer(self) {
        let pending = stored(None::<Spawned<()>>);
        effect(move || {
            if let Some(old) = pending.replace(None) {
                old.cancel();
            }
            if self.toast.get().is_some() {
                let task = set_timeout(Duration::from_millis(3400), move || self.toast.set(None));
                let _ = pending.replace(Some(task));
            }
        });
    }

    fn persist(self) {
        let nav = self.nav;
        let destination = computed(move || nav.with(|nav| nav.destination));
        effect(move || remember(DESTINATION_KEY, destination.get().slug()));
        let settings = computed(move || nav.with(|nav| nav.settings));
        effect(move || remember(SETTINGS_KEY, settings.get().slug()));
        effect(move || {
            let choice = self.theme.get();
            set_color_scheme(appearance(choice));
            remember(THEME_KEY, choice.slug());
        });
        effect(move || remember(DATA_KEY, self.data.get().slug()));
        effect(move || remember(CONN_KEY, self.conn.get().slug()));
        effect(move || remember(MODE_KEY, self.mode.get().slug()));
    }

    fn bind_keys(self) {
        let typing = KeyOptions {
            when: When::NotEditing,
            ..KeyOptions::default()
        };
        let anywhere = KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        };
        let mut keys: BTreeSet<&'static str> = ["/", "?", "`"].into_iter().collect();
        keys.extend(
            Destination::ALL
                .into_iter()
                .filter_map(commands::destination_key),
        );
        for domain in [inbox::KEYS, work::KEYS, plans::KEYS, review::KEYS] {
            keys.extend(domain.iter().copied());
        }
        for key in keys {
            // NSH names a shifted character by its unshifted key.
            let stroke = if key == "?" { "shift+/" } else { key };
            on_key_with(stroke, typing, move |_| self.press_key(key));
        }
        on_key_with("mod+k", anywhere, move |_| {
            if self.is_open(Overlay::Palette) {
                self.close(Overlay::Palette);
            } else {
                self.open_palette();
            }
        });
        on_key_with("escape", anywhere, move |_| self.close_newest());
        on_key_with("mod+shift+m", anywhere, move |_| {
            let mut effects = Vec::new();
            self.assistant.update(|state| effects = state.cycle_model());
            self.run(effects);
        });
    }

    fn toggle_session(&self) {
        self.agents.update(|state| state.toggle_view());
        let session = self.agents.with_untracked(|state| state.session);
        if self.page() != Some(Page::Session(session)) {
            self.open(Page::Session(session));
        }
    }

    fn press_key(&self, key: &str) {
        if self.confirm.get_untracked().is_some()
            || self.nav.with_untracked(|nav| nav.newest().is_some())
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
        let page = self.page();
        let env = self.env();
        let mut effects = None;
        match (self.destination(), page) {
            (Destination::Inbox, None | Some(Page::Question)) => self
                .inbox
                .update(|state| effects = state.key(key, page, env)),
            (Destination::Now, _) => self
                .work
                .update(|state| effects = state.key(key, page, env)),
            (Destination::Plans, _) => self
                .plans
                .update(|state| effects = state.key(key, page, env)),
            (Destination::Review, _) => self
                .review
                .update(|state| effects = state.key(key, page, env)),
            _ => {}
        }
        if let Some(effects) = effects {
            self.run(effects);
        } else if let Some(destination) = Destination::ALL
            .into_iter()
            .find(|destination| commands::destination_key(*destination) == Some(key))
        {
            self.go(destination);
        }
    }
}

fn preference<T>(key: &str, parse: fn(&str) -> Option<T>) -> Option<T> {
    Storage::get_item(key).and_then(|value| parse(&value))
}

// Storage refusal only loses the preference for the next launch.
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
