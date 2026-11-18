//! App signals, persistence and effect dispatch. Domain reducers stay pure; this layer applies
//! their effects in order and owns the confirm and refusal sheets, overlays and toast timing.
//! The live bridge to corrald (`live`) dispatches details and assistant calls, `ops` submits
//! engine operations; `keys` routes the keyboard.

mod attach;
mod devices;
mod keys;
mod live;
mod ops;
mod preview;
mod projects;
mod selection;
mod sidebar;
mod source;

use std::collections::BTreeMap;
use std::time::Duration;

use corral_protocol::assistant::ContextRef;
use nsh_core::prelude::*;
use nsh_core::storage::Storage;

use crate::model::act::RefusalSheet;
use crate::model::commands::Action;
use crate::model::{
    Act, Conn, DataState, Destination, Effect, Env, Form, Id, NavState, Overlay, SettingsSection,
    SidebarPref, Snapshot, ThemeChoice, agents, assistant, daemon, plans, review, settings,
    timeline, usage, work,
};

const DESTINATION_KEY: &str = "corral.destination";
const SETTINGS_KEY: &str = "corral.settings";
const THEME_KEY: &str = "corral.theme";
const SIDEBAR_KEY: &str = "corral.sidebar";

/// Shared app handles. Screens render tracked reads and send domain effects back through `run`.
#[derive(Clone, Copy)]
pub struct AppState {
    pub form: Form,
    pub nav: Signal<NavState>,
    pub data: Signal<DataState>,
    pub feed: source::FeedSource,
    pub theme: Signal<ThemeChoice>,
    /// The transport writes here even while the explicitly selected synthetic feed is shown.
    daemon_feed: Signal<daemon::Feed>,
    /// Invalidates a system picker opened before a data-source change.
    source_revision: Signal<u64>,
    pub ask_draft: Signal<String>,
    /// Grows by one each time Ask opens, so the composer focuses its field.
    pub ask_focus: Signal<u32>,
    pub work: source::StateSource<work::State>,
    pub plans: source::StateSource<plans::State>,
    pub timeline: Signal<timeline::State>,
    pub usage: Signal<usage::State>,
    pub agents: source::StateSource<agents::State>,
    pub review: source::StateSource<review::State>,
    pub settings: source::StateSource<settings::State>,
    pub assistant: source::StateSource<assistant::State>,
    /// The destructive or forced operation, or the session control, waiting for its one local
    /// confirmation.
    pub confirm: Signal<Option<crate::model::act::Confirming>>,
    /// The engine's last refusal and its next steps, until one is pressed or it is closed.
    pub refusal: Signal<Option<RefusalSheet>>,
    /// The real context "Ask about this" attached to the composer; cleared when Ask closes.
    pub ask_context: Signal<Option<ContextRef>>,
    /// Independent per-store screen scopes persisted as `corral.filters`.
    pub filters: Signal<crate::model::projects::Filters>,
    project_context_removed: Signal<Option<Id>>,
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
    /// Operations submitted from here whose receipt has not been shown yet.
    watching: Signal<BTreeMap<Id, Act>>,
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
        let identity = crate::client::requests::new_id();
        let namespace = identity
            .strip_prefix("app-")
            .unwrap_or(&identity)
            .split('-')
            .next()
            .unwrap_or("app");
        let data = signal(data);
        let daemon_feed = signal(feed);
        let preview_feed = signal(daemon::Feed::default());
        let app = AppState {
            form,
            nav: signal(nav),
            data,
            feed: source::FeedSource::new(data, daemon_feed, preview_feed),
            daemon_feed,
            source_revision: signal(0),
            theme: signal(
                preference(THEME_KEY, ThemeChoice::from_slug).unwrap_or(ThemeChoice::System),
            ),
            sidebar: signal(preference(SIDEBAR_KEY, SidebarPref::from_slug).unwrap_or_default()),
            ask_draft: signal(String::new()),
            ask_focus: signal(0),
            work: source::StateSource::new(
                data,
                work::State {
                    ids: crate::model::id::IdSupply::new(format!("{namespace}-wd")),
                    ..Default::default()
                },
                work::State {
                    ids: crate::model::id::IdSupply::new(format!("{namespace}-wf")),
                    ..Default::default()
                },
            ),
            plans: source::StateSource::new(
                data,
                plans::State {
                    ids: crate::model::id::IdSupply::new(format!("{namespace}-pd")),
                    ..Default::default()
                },
                plans::State {
                    ids: crate::model::id::IdSupply::new(format!("{namespace}-pf")),
                    ..Default::default()
                },
            ),
            timeline: signal(timeline::State::default()),
            usage: signal(usage::State::default()),
            agents: source::StateSource::new(
                data,
                agents::State {
                    ids: crate::model::id::IdSupply::new(format!("{namespace}-ad")),
                    ..Default::default()
                },
                agents::State {
                    ids: crate::model::id::IdSupply::new(format!("{namespace}-af")),
                    ..Default::default()
                },
            ),
            review: source::StateSource::new(
                data,
                review::State {
                    ids: crate::model::id::IdSupply::new(format!("{namespace}-rd")),
                    ..Default::default()
                },
                review::State {
                    ids: crate::model::id::IdSupply::new(format!("{namespace}-rf")),
                    ..Default::default()
                },
            ),
            settings: source::StateSource::new(
                data,
                settings::State {
                    ids: crate::model::id::IdSupply::new(format!("{namespace}-sd")),
                    ..Default::default()
                },
                settings::State {
                    ids: crate::model::id::IdSupply::new(format!("{namespace}-sf")),
                    ..Default::default()
                },
            ),
            assistant: source::StateSource::new(
                data,
                assistant::State {
                    ids: crate::model::id::IdSupply::new(format!("{namespace}-cd")),
                    ..Default::default()
                },
                assistant::State {
                    ids: crate::model::id::IdSupply::new(format!("{namespace}-cf")),
                    ..Default::default()
                },
            ),
            confirm: signal(None),
            refusal: signal(None),
            ask_context: signal(None),
            filters: signal(projects::load_filters()),
            project_context_removed: signal(None),
            ask_staged: signal(Vec::new()),
            ask_field: signal(None),
            ask_field_text: signal(String::new()),
            ask_field_error: signal(None),
            root: Owner::current(),
            watching: signal(BTreeMap::new()),
            device_ops: signal(BTreeMap::new()),
            sessions: signal(BTreeMap::new()),
            toast: signal_with(None, SignalEquals::Never),
            hotkey: signal(false),
            share: signal(false),
            lock: signal(false),
            sidebar_peek: signal(false),
            focus_root: element_ref(),
            list_keys: element_ref(),
            key_focus: signal(keys::KeyFocus::default()),
        };
        app.follow_browser_pairing();
        if open_pairing {
            // Phone sections are pushed by a navigation intent after the shell starts.
            set_timeout(Duration::ZERO, move || {
                app.go_settings(SettingsSection::Connection);
            });
        }
        app.persist();
        app.follow_source();
        crate::client::start(app.daemon_feed);
        app.install_toast_timer();
        app.watch_receipts();
        app.follow_sidebar();
        app.follow_projects();
        app.settle_controls();
        app.follow_cached_feed();
        app.watch_device_receipts();
        if form != Form::Phone {
            app.bind_keys();
        }
        app
    }

    /// Tracked environment derived from the selected feed.
    pub fn env(&self) -> Env {
        match self.data.get() {
            DataState::Daemon | DataState::Fixture => self
                .feed
                .with(|feed| crate::model::health::daemon_env(feed, self.form)),
            DataState::Disconnected => Env {
                conn: Conn::Down,
                form: self.form,
            },
        }
    }

    /// Tracked: this source uses the shared daemon model and screen path.
    pub fn daemon(&self) -> bool {
        self.data.get() != DataState::Disconnected
    }
    fn env_untracked(&self) -> Env {
        untracked(|| self.env())
    }

    /// Tracked shell snapshot, derived from the same feed as every screen.
    pub fn snapshot(&self) -> Snapshot {
        if self.data.get() == DataState::Disconnected {
            return crate::model::disconnected::snapshot();
        }
        let mut snapshot = self
            .feed
            .with(|feed| crate::model::health::daemon_snapshot(feed, self.form));
        if self.data.get() == DataState::Fixture {
            snapshot.state = DataState::Fixture;
            snapshot.health.source = Some("Preview · fixture sample data · local only".into());
            snapshot.health.reach = Some("synthetic feed · nothing sent".into());
            snapshot.health.e2e = false;
        }
        snapshot
    }
    /// Tracked current destination, including the owner of the top page.
    pub fn destination(&self) -> Destination {
        self.nav.with(|nav| nav.destination)
    }

    /// Tracked overlay visibility.
    pub fn is_open(&self, overlay: Overlay) -> bool {
        self.nav.with(|nav| nav.is_open(overlay))
    }

    /// Changes destination and clears all pushed pages.
    pub fn go(&self, destination: Destination) {
        let destination = self.workspace_destination(destination);
        self.change(|nav| nav.go(destination));
    }

    /// Opens a Settings section and clears all pushed pages.
    pub fn go_settings(&self, section: SettingsSection) {
        self.change(|nav| nav.go_settings(section));
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

    /// Opens or raises the palette above other overlays.
    pub fn open_palette(&self) {
        self.change(|nav| nav.open(Overlay::Palette));
    }

    /// Closes an overlay; leaving Ask also detaches its contextual chip.
    pub fn close(&self, overlay: Overlay) {
        self.change(|nav| nav.close(overlay));
        if overlay == Overlay::Ask {
            self.ask_context.set(None);
            self.project_context_removed.set(None);
            self.close_attach_field();
        }
    }

    /// Escape priority: confirm sheet, refusal sheet, model list, newest overlay, the selection
    /// on the Agents or Now page in view, the narrow sidebar's labels, then top page.
    pub fn close_newest(&self) {
        if self.confirm.get_untracked().is_some() {
            self.confirm_no();
            return;
        }
        if self.refusal.get_untracked().is_some() {
            self.close_refusal();
            return;
        }
        if self.work.with_untracked(|state| state.menu.is_some()) {
            self.work.update(|state| state.menu = None);
            return;
        }
        if self.agents.with_untracked(|state| state.rewind.is_some()) {
            self.agents.update(|state| state.rewind = None);
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
        if self.clear_page_selection() {
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

    /// Selects the transport or an isolated, explicitly chosen synthetic feed.
    pub fn set_data(&self, state: DataState) {
        if state == self.data.get_untracked() && state != DataState::Fixture {
            return;
        }
        // Dispose old-source views before draft observers see the new source's empty composer.
        batch(|| {
            self.source_revision
                .update(|revision| *revision = revision.wrapping_add(1));
            self.close(Overlay::Ask);
            self.hotkey.set(false);
            self.share.set(false);
            self.lock.set(false);
            self.confirm.set(None);
            self.refusal.set(None);
            let uploads = self.ask_staged.with_untracked(|staged| {
                staged
                    .iter()
                    .filter_map(|item| match item {
                        assistant::Staged::Upload { upload, .. } => Some(upload.clone()),
                        assistant::Staged::Link { .. } => None,
                    })
                    .collect::<Vec<_>>()
            });
            for upload in uploads {
                self.unattach(&upload);
            }
            self.ask_staged.set(Vec::new());
            self.ask_draft.set(String::new());
            self.ask_context.set(None);
            self.project_context_removed.set(None);
            self.change(|nav| {
                nav.pages.clear();
                nav.selection = Default::default();
            });
            if state == DataState::Fixture {
                let local = jiff::Zoned::now();
                self.feed.preview.set(crate::model::preview::feed(
                    local.timestamp().as_second().max(0) as u64,
                    local.date().to_string(),
                    local.strftime("%H:%M").to_string(),
                    Some(local.time_zone().clone()),
                ));
            }
            self.data.set(state);
        });
    }

    /// Only an actual pairing transition may take a disconnected browser live.
    fn follow_browser_pairing(self) {
        let paired = computed(move || {
            self.daemon_feed.with(|feed| {
                matches!(
                    feed.browser_pairing,
                    crate::model::devices::BrowserPairing::Paired { .. }
                )
            })
        });
        let failed = computed(move || {
            self.daemon_feed
                .with(|feed| matches!(feed.device, crate::model::devices::DeviceLink::Failed(_)))
        });
        effect(move || {
            if paired.get() && self.data.get_untracked() == DataState::Disconnected {
                untracked(|| self.set_data(DataState::Daemon));
            }
        });
        effect(move || {
            if failed.get()
                && self.data.get_untracked() == DataState::Daemon
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
        let cached = computed(move || self.daemon_feed.with(cached_namespace));
        effect(move || {
            if cached.get() && self.data.get_untracked() == DataState::Disconnected {
                untracked(|| self.set_data(DataState::Daemon));
            }
        });
    }

    /// Shows a receipt; repeating the same text also restarts its root-owned delay.
    pub fn toast(&self, text: impl Into<String>) {
        self.toast.set(Some(text.into()));
    }

    /// Applies reducer effects in source order through the selected feed's driver.
    pub fn run(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Toast(text) => self.toast(text),
                Effect::Go(destination) => self.go(destination),
                Effect::GoSettings(section) => self.go_settings(section),
                Effect::Back => self.back(),
                Effect::CloseAsk => self.close(Overlay::Ask),
                other => self.run_live(other),
            }
        }
    }

    /// Runs a local state reducer and dispatches its effects.
    pub fn act<S: Clone + 'static>(
        &self,
        state: impl Into<source::StateHandle<S>>,
        reduce: impl FnOnce(&mut S, Env) -> Vec<Effect>,
    ) {
        let env = self.env_untracked();
        let mut effects = Vec::new();
        state.into().update(|state| effects = reduce(state, env));
        self.run(effects);
    }

    /// Executes a model palette action after closing the palette.
    pub fn run_command(&self, action: Action) {
        self.close(Overlay::Palette);
        match action {
            Action::Go(destination) => self.go(destination),
            Action::Settings(section) => self.go_settings(section),
            Action::OpenLive(live) => self.open_live(live),
            Action::Project(id) => self.open_project(id),
            Action::PreviewNoProjects => self.preview_no_projects(),
            Action::Ask(text) => match text {
                Some(text) => self.open_ask_with(&text),
                None => self.open_ask(),
            },
            Action::ToggleSession => self.toggle_session(),
            Action::Theme(theme) => self.set_theme(theme),
            Action::Data(data) => self.set_data(data),
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
            Action::Work(press) => self.reduce(self.work, |state, feed, _, env| {
                state.press(press, feed, env)
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
        effect(move || remember(SIDEBAR_KEY, self.sidebar.get().slug()));
    }

    /// Switches the selected session between its shared transcript and terminal views.
    fn toggle_session(&self) {
        if !untracked(|| self.daemon()) {
            return;
        }
        let selected = self.nav.with_untracked(|nav| nav.selection.session.clone());
        let Some(session) = selected else {
            return;
        };
        self.feed.with_untracked(|feed| {
            self.agents
                .update(|state| state.toggle_view(feed, session.as_str()))
        });
        let live = crate::model::Live::Session(session);
        if self.nav.with_untracked(|nav| nav.live() != Some(&live)) {
            self.open_live(live);
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
        feed.capabilities.insert(corral_protocol::TASKS_V1.into());
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
