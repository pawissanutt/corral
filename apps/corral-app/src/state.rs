//! App signals, persistence and effect dispatch. Domain reducers stay pure; this layer applies
//! their effects in order and owns transient confirmations, overlays and toast timing. The live
//! bridge to corrald (`live`) dispatches typed commands, details and assistant calls; `keys`
//! routes the keyboard.

mod attach;
mod confirmation;
mod devices;
mod keys;
mod live;
mod sidebar;

use std::collections::BTreeMap;
use std::time::Duration;

use corral_protocol::assistant::ContextRef;
use nsh_core::prelude::*;
use nsh_core::storage::Storage;

use crate::model::act::{Confirmable, Confirmation};
use crate::model::commands::Action;
use crate::model::{
    AskTarget, Conn, DataMode, DataState, Destination, Effect, Env, Form, Id, NavState, Overlay,
    Page, Risk, SettingsSection, SidebarPref, Snapshot, ThemeChoice, agents, assistant, commands,
    daemon, inbox, plans, review, settings, timeline, usage, work,
};

const DESTINATION_KEY: &str = "corral.destination";
const SETTINGS_KEY: &str = "corral.settings";
const THEME_KEY: &str = "corral.theme";
const CONN_KEY: &str = "corral.conn";
const MODE_KEY: &str = "corral.mode";
const NOW_LAYOUT_KEY: &str = "corral.now.layout";
const SIDEBAR_KEY: &str = "corral.sidebar";

/// Shared app handles. Screens render tracked reads and send domain effects back through `run`.
#[derive(Clone, Copy)]
pub struct AppState {
    pub form: Form,
    pub nav: Signal<NavState>,
    pub data: Signal<DataState>,
    pub feed: Signal<daemon::Feed>,
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
    /// Fresh confirmation of a fixture preview command, with an optional device or idea name.
    pub confirm: Signal<Option<(Risk, Option<String>)>>,
    /// Fresh confirmation of a live command, waiting for corrald's preview and this device.
    pub confirmation: Signal<Option<Confirmation>>,
    /// The real context "Ask about this" attached to the composer; cleared when Ask closes.
    pub ask_context: Signal<Option<ContextRef>>,
    /// What is staged for the next message, shared by Ask and docked composers like the draft.
    /// In memory only; it outlives a closed Ask so a picker answering late still lands.
    pub ask_staged: Signal<Vec<assistant::Staged>>,
    /// The inline link or path field open in the composer foot, if any.
    pub ask_field: Signal<Option<assistant::AttachField>>,
    /// The open field's text; never persisted, never logged.
    pub ask_field_text: Signal<String>,
    /// Why the open field's text was not attached.
    pub ask_field_error: Signal<Option<String>>,
    /// The app's root owner: a picker's answer is awaited under it, so closing Ask or the
    /// sheet while the system picker is up does not drop the picked files.
    pub root: Owner,
    /// Live commands sent from here whose receipt has not been shown yet, and where each
    /// stands with this device's confirmation.
    watching: Signal<BTreeMap<Id, (Confirmable, live::Watch)>>,
    /// Revision under which the open device-bound preview was requested.
    confirmation_revision: Signal<u64>,
    device_ops: Signal<BTreeMap<Id, devices::DeviceOperation>>,
    /// Assistant session operations by their caller key, to the client's correlation.
    sessions: Signal<BTreeMap<Id, Id>>,
    /// A receipt that expires 3.4 seconds after the most recent toast call.
    pub toast: Signal<Option<String>>,
    /// Later UI phases render the global hotkey window when true.
    pub hotkey: Signal<bool>,
    /// Later UI phases render the share-sheet demo when true.
    pub share: Signal<bool>,
    /// Later UI phases render the lock-screen push demo when true.
    pub lock: Signal<bool>,
    /// Persisted wide-layout sidebar choice; a narrow window or grid always folds it.
    pub sidebar: Signal<SidebarPref>,
    /// The labels opened over the content from the rail while narrow; never persisted.
    sidebar_peek: Signal<bool>,
    /// On the app's root view: the keyboard focus being anywhere in the app.
    pub focus_root: ElementRef,
    /// On the destination's keyboard list (Now's, Inbox's): its keys act while the focus is in
    /// it, or nowhere.
    pub list_keys: ElementRef,
    key_focus: Signal<keys::KeyFocus>,
}

impl AppState {
    /// Restores preferences, starts root-owned persistence and toast timing, and binds non-phone keys.
    pub fn install(form: Form) -> AppState {
        let pairing_link = crate::client::pairing_link();
        let mut nav = NavState::new(form);
        if let Some(destination) = preference(DESTINATION_KEY, Destination::from_slug) {
            nav.destination = destination;
        }
        if let Some(section) = preference(SETTINGS_KEY, SettingsSection::from_slug) {
            nav.settings = section;
        }
        let (data, feed) = startup(
            &mut nav,
            crate::client::pairing_supported(),
            crate::client::configured(),
            pairing_link,
        );
        let open_pairing = nav.section_opens > 0;
        let app = AppState {
            form,
            nav: signal(nav),
            data: signal(data),
            feed: signal(feed),
            theme: signal(
                preference(THEME_KEY, ThemeChoice::from_slug).unwrap_or(ThemeChoice::System),
            ),
            sidebar: signal(preference(SIDEBAR_KEY, SidebarPref::from_slug).unwrap_or_default()),
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
            confirmation: signal(None),
            ask_context: signal(None),
            ask_staged: signal(Vec::new()),
            ask_field: signal(None),
            ask_field_text: signal(String::new()),
            ask_field_error: signal(None),
            root: Owner::current(),
            watching: signal(BTreeMap::new()),
            confirmation_revision: signal(0),
            device_ops: signal(BTreeMap::new()),
            sessions: signal(BTreeMap::new()),
            confirm: signal(None),
            toast: signal_with(None, SignalEquals::Never),
            hotkey: signal(false),
            share: signal(false),
            lock: signal(false),
            sidebar_peek: signal(false),
            focus_root: element_ref(),
            list_keys: element_ref(),
            key_focus: signal(keys::KeyFocus::default()),
        };
        crate::client::start(app.feed);
        app.follow_browser_pairing();
        if open_pairing {
            // Phone sections are pushed by a navigation intent after the shell starts.
            set_timeout(Duration::ZERO, move || {
                app.go_settings(SettingsSection::Connection);
            });
        }
        app.persist();
        app.install_toast_timer();
        app.watch_receipts();
        app.follow_sidebar();
        app.latch_previews();
        app.follow_cached_feed();
        app.follow_authority();
        app.watch_device_receipts();
        if form != Form::Phone {
            app.bind_keys();
        }
        app
    }

    /// Tracked environment for any domain's reducer or view builder. The preview's demo
    /// connection and data-mode switches apply only to the fixture preview.
    pub fn env(&self) -> Env {
        match self.data.get() {
            DataState::Daemon => self
                .feed
                .with(|feed| crate::model::health::daemon_env(feed, self.form)),
            DataState::Fixture => Env {
                conn: self.conn.get(),
                mode: self.mode.get(),
                form: self.form,
            },
            // Nothing is served without a connection; actions refuse rather than queue.
            DataState::Disconnected => Env {
                conn: Conn::Down,
                mode: DataMode::Live,
                form: self.form,
            },
        }
    }

    /// Tracked: the current source is the daemon rather than the mockup.
    pub fn daemon(&self) -> bool {
        self.data.get() == DataState::Daemon
    }

    fn env_untracked(&self) -> Env {
        untracked(|| self.env())
    }

    /// Tracked shell snapshot, with the current Inbox reducer's needs-you count.
    pub fn snapshot(&self) -> Snapshot {
        if self.daemon() {
            return self
                .feed
                .with(|feed| crate::model::health::daemon_snapshot(feed, self.form));
        }
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

    /// Opens Ask without discarding its context or draft. Disconnected startup still opens
    /// the drawer so it can explain the missing connection without showing fixture data.
    pub fn open_ask(&self) {
        use crate::model::Availability;
        let refusal = untracked(|| {
            match self.data.get() {
            DataState::Daemon => self.feed.with(|feed| {
                match crate::model::health::capability_availability(
                    feed,
                    corral_protocol::app::ASSISTANT_V1,
                ) {
                    Availability::Ready => None,
                    Availability::Missing(_) => Some(
                        "This corrald does not offer Ask to the app (assistant.v1 is not advertised).",
                    ),
                    Availability::Connecting if feed.link == crate::model::daemon::Link::Down => {
                        Some("corrald is not reachable. Ask opens once a session with it is open.")
                    }
                    Availability::Connecting => Some("Connecting to corrald; Ask opens once it answers."),
                }
            }),
            DataState::Disconnected => None,
            _ => None,
        }
        });
        if let Some(refusal) = refusal {
            self.toast(refusal);
            return;
        }
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
            self.ask_context.set(None);
            self.close_attach_field();
        }
    }

    /// Escape priority: confirmation, model list, newest overlay, the narrow sidebar's labels,
    /// then top page.
    pub fn close_newest(&self) {
        if self.confirm.get_untracked().is_some() || self.confirmation.get_untracked().is_some() {
            self.confirm_no();
            return;
        }
        if self.work.with_untracked(|state| state.menu.is_some()) {
            self.work.update(|state| state.menu = None);
            return;
        }
        if self.ask_field.with_untracked(Option::is_some) {
            self.close_attach_field();
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
        if !self.close_sidebar_peek() {
            self.back();
        }
    }

    /// Changes the persisted appearance and system color scheme.
    pub fn set_theme(&self, choice: ThemeChoice) {
        self.theme.set(choice);
    }

    /// Changes the source without carrying the fixture preview's dialogs and demos out of it.
    pub fn set_data(&self, state: DataState) {
        if state != DataState::Fixture {
            self.close(Overlay::Ask);
            self.confirm.set(None);
            self.hotkey.set(false);
            self.share.set(false);
            self.lock.set(false);
        }
        // A live confirmation never outlives its source.
        self.invalidate_confirmations();
        self.data.set(state);
    }

    /// A browser that finishes pairing shows its live session at once, from whichever source
    /// it paired: the stored identity is what a reload resumes anyway. A browser whose stored
    /// pairing turns out to be gone (the client clears its marker) is unpaired, so it returns to
    /// Connection's pairing form instead of waiting on a session that cannot start.
    fn follow_browser_pairing(self) {
        let paired = computed(move || {
            self.feed.with(|feed| {
                matches!(
                    feed.browser_pairing,
                    crate::model::devices::BrowserPairing::Paired { .. }
                )
            })
        });
        let failed = computed(move || {
            self.feed
                .with(|feed| matches!(feed.device, crate::model::devices::DeviceLink::Failed(_)))
        });
        // Each transition follows only its own change, so an explicit later choice of source
        // is never overridden by the other.
        effect(move || {
            if paired.get() && !untracked(|| self.daemon()) {
                untracked(|| self.set_data(DataState::Daemon));
            }
        });
        effect(move || {
            if failed.get()
                && untracked(|| self.daemon())
                && crate::client::pairing_supported()
                && !crate::client::configured()
            {
                untracked(|| {
                    self.go_settings(SettingsSection::Connection);
                    self.set_data(DataState::Disconnected);
                });
            }
        });
    }

    /// A restored namespace remains daemon data after link loss clears its live session.
    /// Observe only cache identity transitions so an explicit fixture choice stays explicit.
    fn follow_cached_feed(self) {
        let cached = computed(move || self.feed.with(cached_namespace));
        effect(move || {
            if cached.get() && self.data.get_untracked() == DataState::Disconnected {
                untracked(|| self.set_data(DataState::Daemon));
            }
        });
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

    /// Opens the fixture preview's fresh confirmation; corrald commands go through `dispatch`.
    pub fn confirm(&self, risk: Risk, name: Option<String>) {
        if untracked(|| self.daemon()) {
            // A fixture reducer's sample command has no live target.
            self.toast("The fixture preview's sample command is not sent to corrald.");
            return;
        }
        if self.env_untracked().conn.refuses() {
            self.toast("Refused: needs a live connection. Nothing was queued.");
            return;
        }
        self.confirm.set(Some((risk, name)));
    }

    /// Confirms what the sheet shows: a live command with corrald's fresh preview proof, or
    /// the fixture preview's sample with its receipt.
    pub fn confirm_yes(&self) {
        if self.confirmation.get_untracked().is_some() {
            self.confirm_live();
            return;
        }
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
            self.env_untracked().device()
        ));
    }

    /// Cancels the confirmation without sending or applying anything.
    pub fn confirm_no(&self) {
        self.confirm.set(None);
        self.cancel_live();
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
                live => self.run_live(live),
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
        let env = self.env_untracked();
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
            Action::OpenLive(live) => self.open_live(live),
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
            Action::Work(press) => self.act(self.work, |state, env| {
                state.press(press, &self.feed.get_untracked(), env)
            }),
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
        effect(move || remember(CONN_KEY, self.conn.get().slug()));
        effect(move || remember(MODE_KEY, self.mode.get().slug()));
        effect(move || remember(SIDEBAR_KEY, self.sidebar.get().slug()));
    }

    /// The fixture session's transcript/terminal switch, or the live selected session's.
    fn toggle_session(&self) {
        match self.data.get_untracked() {
            DataState::Daemon => {
                let selected = self.nav.with_untracked(|nav| nav.selection.session.clone());
                let Some(session) = selected else {
                    return;
                };
                self.agents.update(|state| state.toggle_live_view());
                let live = crate::model::Live::Session(session);
                if self.nav.with_untracked(|nav| nav.live() != Some(&live)) {
                    self.open_live(live);
                }
                return;
            }
            DataState::Disconnected => return,
            DataState::Fixture => {}
        }
        self.agents.update(|state| state.toggle_view());
        let session = self.agents.with_untracked(|state| state.session);
        if self.page() != Some(Page::Session(session)) {
            self.open(Page::Session(session));
        }
    }
}

fn cached_namespace(feed: &daemon::Feed) -> bool {
    feed.store.is_some()
        && (feed.session.is_some() || !feed.capabilities.is_empty() || feed.board.complete)
}

fn startup(
    nav: &mut NavState,
    browser: bool,
    configured: bool,
    pairing_link: Result<Option<String>, &'static str>,
) -> (DataState, daemon::Feed) {
    let mut feed = daemon::Feed::default();
    match pairing_link {
        Ok(code) => feed.pairing_code = code,
        Err(line) => {
            feed.browser_pairing = crate::model::devices::BrowserPairing::Blocked(line);
        }
    }
    // The fixture preview is never a fallback and never outlives the launch it was chosen in:
    // a restart is live or disconnected, so its healthy-looking facts cannot pass for live.
    let data = if feed.browser_pairing_pending() || (browser && !configured) {
        nav.go_settings(SettingsSection::Connection);
        DataState::Disconnected
    } else if configured {
        DataState::Daemon
    } else {
        DataState::Disconnected
    };
    (data, feed)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::devices::{BrowserPairing, PairBrowser};

    #[test]
    fn offline_cached_namespace_survives_session_invalidation() {
        let mut feed = daemon::Feed {
            store: Some("/workspace/.corral".into()),
            ..Default::default()
        };
        feed.capabilities
            .insert(corral_protocol::app::ACT_V1.into());
        feed.mark_stale();
        feed.link = daemon::Link::Down;
        assert!(feed.session.is_none());
        assert!(cached_namespace(&feed));
        feed.capabilities.clear();
        assert!(!cached_namespace(&feed));
        feed.board.complete = true;
        assert!(cached_namespace(&feed));
        feed.store = None;
        assert!(!cached_namespace(&feed));
    }

    #[test]
    fn unpaired_browser_opens_connection_instead_of_restoring_mockup() {
        let mut nav = NavState::new(Form::Desktop);
        nav.destination = Destination::Now;
        let (data, _) = startup(&mut nav, true, false, Ok(None));
        assert_eq!(nav.destination, Destination::Settings);
        assert_eq!(nav.settings, SettingsSection::Connection);
        assert_eq!(data, DataState::Disconnected);
    }

    #[test]
    fn restart_never_restores_the_fixture_preview() {
        for form in [Form::Desktop, Form::Terminal] {
            let mut nav = NavState::new(form);
            let (data, _) = startup(&mut nav, false, false, Ok(None));
            assert_eq!(data, DataState::Disconnected);
            let (data, _) = startup(&mut nav, false, true, Ok(None));
            assert_eq!(data, DataState::Daemon);
        }
    }

    #[test]
    fn explicit_pairing_link_overrides_old_identity_and_preferences_in_every_form() {
        for form in [Form::Desktop, Form::Terminal, Form::Phone] {
            let mut nav = NavState::new(form);
            let (_, feed) = startup(
                &mut nav,
                true,
                true,
                Ok(Some("corral-pair-1.fixture".into())),
            );
            assert_eq!(nav.destination, Destination::Settings);
            assert_eq!(nav.settings, SettingsSection::Connection);
            assert_eq!(nav.section_opens, 1);
            assert_eq!(feed.pairing_code.as_deref(), Some("corral-pair-1.fixture"));
            assert!(feed.browser_pairing_pending());
            assert_eq!(feed.browser_pairing, BrowserPairing::Idle);
            assert!(matches!(
                crate::model::devices::pair_browser(&feed),
                PairBrowser::Form { .. }
            ));
            assert_eq!(feed.link, daemon::Link::Connecting);
        }
    }

    #[test]
    fn uncleared_link_is_visibly_blocked_and_normal_startup_stays_unchanged() {
        let mut nav = NavState::new(Form::Phone);
        let (_, feed) = startup(&mut nav, true, true, Err("cannot clear fragment"));
        assert_eq!(nav.settings, SettingsSection::Connection);
        assert!(feed.browser_pairing_pending());
        assert_eq!(feed.pairing_code, None);
        assert_eq!(
            crate::model::devices::pair_browser(&feed),
            PairBrowser::Blocked {
                line: "cannot clear fragment"
            },
        );
        let (data, feed) = startup(&mut nav, true, true, Ok(None));
        assert_eq!(data, DataState::Daemon);
        assert!(!feed.browser_pairing_pending());
    }
}
