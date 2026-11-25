//! App signals, persistence and effect dispatch. Domain reducers stay pure; this layer applies
//! their effects in order and owns the confirm and refusal sheets, overlays and toast timing.
//! The live bridge to corrald (`live`) dispatches details and assistant calls, `ops` submits
//! engine operations; `keys` routes the keyboard.

mod address;
mod attach;
mod commands;
mod config;
mod devices;
mod dock;
mod finder;
mod keys;
mod live;
mod ops;
mod pane;
mod preview;
mod projects;
mod selection;
mod sidebar;
mod source;
mod switcher;
mod system_page;

use std::collections::BTreeMap;
use std::time::Duration;

use corral_protocol::assistant::ContextRef;
use nsh_core::prelude::*;
use nsh_core::storage::Storage;

use crate::model::act::RefusalSheet;
use crate::model::{
    Act, Conn, DataState, Destination, Effect, Env, Form, Id, NavState, Overlay, SettingsSection,
    SidebarPref, Snapshot, ThemeChoice, agents, artifacts, assistant, daemon, review, session,
    settings, system, timeline, usage, work,
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
    pub(crate) source_revision: Signal<u64>,
    /// The Agents switcher is open (Mod+G or its trigger).
    pub switcher: Signal<bool>,
    /// This device's pins and last opened conversation per daemon store, saved as
    /// `corral.agents`; the explicit preview keeps its own in memory.
    pins: Signal<agents::Pins>,
    preview_pins: Signal<agents::Pins>,
    /// The New assistant operation whose chat Ask is keyed to once corrald answers with it.
    switch_new: Signal<Option<Id>>,
    /// Grows by one each time Ask opens, so the composer focuses its field.
    pub ask_focus: Signal<u32>,
    /// The test id of a control a direct key asked to take the keyboard focus (`Effect::Focus`),
    /// until it has taken it or the next commit has passed.
    pub focus_to: Signal<Option<&'static str>>,
    /// Home's docked composer field while it has the keyboard focus, which Escape takes off it.
    pub docked_field: Signal<Option<ElementRef>>,
    pub work: source::StateSource<work::State>,
    pub timeline: Signal<timeline::State>,
    pub usage: Signal<usage::State>,
    pub artifacts: Signal<artifacts::State>,
    /// Activity › System: the level, pick, overlay and filter.
    pub system: Signal<system::State>,
    /// A task page's Changes view (Diff or Impact) and the Impact's pick.
    pub impact: Signal<review::impact::State>,
    pub agents: source::StateSource<agents::State>,
    /// Every session page's UI state (tab, draft, this device's sends), kept for the app's life.
    pub session: source::StateSource<session::State>,
    pub review: source::StateSource<review::State>,
    /// Home's selected row or card.
    pub home: Signal<crate::model::home::State>,
    /// The Tasks list's open folds.
    pub tasks: Signal<crate::model::tasks::State>,
    pub settings: source::StateSource<settings::State>,
    pub assistant: source::StateSource<assistant::State>,
    /// The destructive or forced operation, or the session control, waiting for its one local
    /// confirmation.
    pub confirm: Signal<Option<crate::model::act::Confirming>>,
    /// The engine's last refusal and its next steps, until one is pressed or it is closed.
    pub refusal: Signal<Option<RefusalSheet>>,
    /// The real context "Ask about this" attached to the composer; cleared when Ask closes.
    pub ask_context: Signal<Option<ContextRef>>,
    dock_ui: dock::DockUi,
    /// The one project scope per daemon store, persisted as `corral.scope`.
    pub scopes: Signal<crate::model::projects::Scopes>,
    /// A scope asked for before the daemon named its store.
    scope_pending: Signal<Option<crate::model::projects::Scope>>,
    /// Whether a list page shows its inspector beside the rows (`i`); hidden, a row opens its
    /// page on one press.
    pub inspector: Signal<bool>,
    project_context_removed: Signal<Option<Id>>,
    /// What is staged for each session's next message, shared by Ask and the session page like
    /// the draft. In memory only; it outlives a closed Ask so a picker answering late still lands.
    pub ask_staged: Signal<BTreeMap<Id, Vec<assistant::Staged>>>,
    /// The inline link or path field open in a session composer's foot, if any.
    pub ask_field: Signal<Option<(Id, assistant::AttachField)>>,
    /// The open field's text; never persisted, never logged.
    pub ask_field_text: Signal<String>,
    /// Why the open field's text was not attached.
    pub ask_field_error: Signal<Option<String>>,
    /// The app's root owner: a picker's answer is awaited under it, so closing Ask or the
    /// sheet while the system picker is up does not drop the picked files.
    pub root: Owner,
    /// Operations submitted from here whose receipt has not been shown yet.
    watching: Signal<BTreeMap<Id, Act>>,
    /// Operations corrald answered `running`, by their submission's key: the id `op.result`
    /// follows until the outcome is in, kept here so a lost reply is asked for again.
    following: Signal<BTreeMap<Id, String>>,
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
    pub pane: pane::Pane,
    /// The labels opened over the content from the rail while narrow; never persisted.
    sidebar_peek: Signal<bool>,
    /// On the app's root view: the keyboard focus being anywhere in the app.
    pub focus_root: ElementRef,
    /// On the place's keyboard list (Home's, Tasks', Sessions'): its keys act while the focus is
    /// in it, or nowhere.
    pub list_keys: ElementRef,
    /// The list row, by its test id, that holds the keyboard focus: the list's Enter opens the
    /// selected row's page only while the focus is on that row (or nowhere), so Enter on a
    /// focused fold or another row presses that control.
    pub focused_row: Signal<Option<String>>,
    /// The Settings section open on the phone's stack, which its shell keeps; the address
    /// names it.
    pub phone_section: Signal<Option<SettingsSection>>,
    /// Press-opened menus open now: the terminal's key line gives a menu's keys while any is.
    pub menus: Signal<u32>,
    key_focus: Signal<keys::KeyFocus>,
}

impl AppState {
    /// Restores preferences, starts root-owned persistence and toast timing, and binds the keys
    /// (on the phone, only the palette's).
    pub fn install(form: Form) -> AppState {
        let pairing_link = crate::client::pairing_link();
        let mut nav = NavState::new(form);
        if let Some(destination) = preference(DESTINATION_KEY, Destination::from_slug) {
            nav.destination = destination;
        }
        if let Some(section) = preference(SETTINGS_KEY, SettingsSection::from_slug) {
            nav.settings = section;
        }
        // The address a browser was opened at names the place over the stored destination.
        // A project's address is Home scoped to it; the scope waits for the daemon's store.
        let mut launch_scope = None;
        let mut launch_system = system::State::default();
        let mut launch_impact = review::impact::State::default();
        if let Some(place) = address::launched() {
            place.restore_impact(&mut launch_impact);
            if let crate::model::address::Place::SystemView(view) = place {
                launch_system.view = view;
            }
            if let crate::model::address::Place::Project(id) = &place {
                launch_scope = Some(crate::model::projects::Scope::Project(id.clone()));
            }
            place.open(&mut nav);
        }
        let (data, feed) = startup(
            &mut nav,
            crate::client::pairing_supported(),
            crate::client::configured(),
            pairing_link,
        );
        let open_section = (nav.section_opens > 0).then_some(nav.settings);
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
            pane: pane::Pane::new(),
            switcher: signal(false),
            pins: signal(switcher::load_pins()),
            preview_pins: signal(agents::Pins::default()),
            switch_new: signal(None),
            ask_focus: signal(0),
            focus_to: signal(None),
            docked_field: signal(None),
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
            timeline: signal(timeline::State::default()),
            usage: signal(usage::State::default()),
            artifacts: signal(artifacts::State::default()),
            system: signal(launch_system),
            impact: signal(launch_impact),
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
            session: source::StateSource::new(
                data,
                session::State {
                    ids: crate::model::id::IdSupply::new(format!("{namespace}-ed")),
                    ..Default::default()
                },
                session::State {
                    ids: crate::model::id::IdSupply::new(format!("{namespace}-ef")),
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
            home: signal(Default::default()),
            tasks: signal(Default::default()),
            confirm: signal(None),
            refusal: signal(None),
            ask_context: signal(None),
            dock_ui: dock::DockUi::new(),
            scopes: signal(projects::load_scopes()),
            scope_pending: signal(launch_scope),
            inspector: signal(true),
            project_context_removed: signal(None),
            ask_staged: signal(BTreeMap::new()),
            ask_field: signal(None),
            ask_field_text: signal(String::new()),
            ask_field_error: signal(None),
            root: Owner::current(),
            watching: signal(BTreeMap::new()),
            following: signal(BTreeMap::new()),
            device_ops: signal(BTreeMap::new()),
            sessions: signal(BTreeMap::new()),
            toast: signal_with(None, SignalEquals::Never),
            hotkey: signal(false),
            share: signal(false),
            lock: signal(false),
            sidebar_peek: signal(false),
            focus_root: element_ref(),
            list_keys: element_ref(),
            focused_row: signal(None),
            phone_section: signal(None),
            menus: signal(0),
            key_focus: signal(keys::KeyFocus::default()),
        };
        app.follow_browser_pairing();
        if let Some(section) = open_section {
            // Phone sections are pushed by a navigation intent after the shell starts.
            set_timeout(Duration::ZERO, move || {
                app.go_settings(section);
            });
        }
        app.persist();
        app.follow_address();
        app.follow_source();
        crate::client::start(app.daemon_feed);
        app.install_toast_timer();
        app.watch_receipts();
        app.follow_sidebar();
        app.follow_projects();
        app.follow_sessions();
        app.follow_agents();
        app.follow_dock();
        app.settle_config();
        app.follow_cached_feed();
        app.watch_device_receipts();
        if form == Form::Phone {
            app.bind_phone_keys();
        } else {
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
        let destination = untracked(|| self.workspace_destination(destination));
        self.change(|nav| nav.go(destination));
    }

    /// Opens a place: its own page, or for Activity the page last shown there.
    pub fn go_area(&self, area: crate::model::nav::Area) {
        self.go(self.nav.with_untracked(|nav| nav.page_of(area)));
    }

    /// Shows or hides the inspector beside every list page.
    pub fn toggle_inspector(&self) {
        self.inspector.update(|shown| *shown = !*shown);
    }

    /// Shows a tab of the task page on top.
    pub fn set_task_tab(&self, tab: crate::model::nav::TaskTab) {
        self.nav.update(|nav| nav.task_tab = tab);
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
        self.set_ask_draft(text.to_owned());
        self.open_ask();
    }

    /// Opens or raises the palette above other overlays.
    pub fn open_palette(&self) {
        self.change(|nav| nav.open(Overlay::Palette));
    }

    /// Closes an overlay; leaving Ask also detaches its contextual chip and closes the side
    /// pane, so a session tab never takes Ask's place unasked.
    pub fn close(&self, overlay: Overlay) {
        let pane = overlay == Overlay::Ask
            && crate::model::dock::supported(self.form)
            && self.nav.with_untracked(|nav| nav.is_open(overlay));
        self.change(|nav| nav.close(overlay));
        if overlay == Overlay::Ask {
            self.ask_context.set(None);
            self.project_context_removed.set(None);
            self.close_attach_field();
        }
        if pane {
            self.hide_dock();
        }
    }

    /// Escape priority: confirm sheet, refusal sheet, the Agents switcher, model list, the operation composer, newest
    /// overlay, the selection on the Sessions page in view, the narrow sidebar's labels,
    /// then top page.
    pub fn close_newest(&self) {
        if self.confirm.get_untracked().is_some() {
            self.confirm_no();
            return;
        }
        if self.refusal.get_untracked().is_some() {
            self.close_refusal();
            return;
        }
        if self.switcher.get_untracked() {
            self.close_agents();
            return;
        }
        if self.work.with_untracked(|state| state.menu.is_some()) {
            self.work.update(|state| state.menu = None);
            return;
        }
        if self.work.with_untracked(|state| state.composer.is_some()) {
            self.work.update(|state| state.composer = None);
            return;
        }
        if self.agents.with_untracked(|state| state.rewind.is_some()) {
            self.agents.update(|state| state.rewind = None);
            return;
        }
        if self.agents.with_untracked(|state| state.start.open) {
            let filter = untracked(|| self.scope_filter());
            self.reduce(self.agents, |state, feed, _, _| {
                state.act(
                    agents::Action::Start(agents::StartAction::Close),
                    feed,
                    &filter,
                )
            });
            return;
        }
        if self.ask_field.with_untracked(Option::is_some) {
            self.close_attach_field();
            return;
        }
        if let Some(overlay) = self.nav.with_untracked(NavState::newest) {
            self.close(overlay);
            return;
        }
        if self.dismiss_dock_overlay() || self.escape_pane() {
            return;
        }
        // Escape leaves Home's docked composer, so Home's keys walk its cards again.
        if let Some(field) = self.docked_field.get_untracked() {
            self.docked_field.set(None);
            field.blur();
            return;
        }
        if self.clear_page_selection() || self.escape_session() {
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
            self.switcher.set(false);
            self.hotkey.set(false);
            self.share.set(false);
            self.lock.set(false);
            self.confirm.set(None);
            // A lens draft or receipt belongs to the source it was made on: Send must not
            // file a sample draft as a real task.
            self.system.update(|state| {
                state.draft = None;
                state.filed = None;
            });
            // Another source has other operations.
            self.following.set(BTreeMap::new());
            self.refusal.set(None);
            let uploads = self.ask_staged.with_untracked(|staged| {
                staged
                    .values()
                    .flatten()
                    .filter_map(|item| match item {
                        assistant::Staged::Upload { upload, .. } => Some(upload.clone()),
                        assistant::Staged::Link { .. } => None,
                    })
                    .collect::<Vec<_>>()
            });
            for upload in uploads {
                self.unattach(None, &upload);
            }
            self.ask_staged.set(BTreeMap::new());
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
                Effect::FindArtifacts(task) => self.find_artifacts(task),
                Effect::OpenArtifact { reference, section } => {
                    self.artifacts_act(crate::model::artifacts::Action::Cite {
                        reference,
                        section,
                    });
                }
                Effect::Focus(id) => {
                    let focus_to = self.focus_to;
                    focus_to.set(Some(id));
                    after_answer(move || focus_to.set(None));
                }
                // The session's page state is this device's, for the sample as for corrald.
                Effect::Answer { session, asked } => {
                    let action = crate::model::session::Action::Dock(
                        crate::model::session::DockAction::Answer { session, asked },
                    );
                    self.reduce(self.session, move |state, feed, _, _| {
                        state.act(action, feed)
                    });
                }
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

    /// Applies a navigation, unless it leaves Configuration with unsaved edits: then the
    /// confirm sheet asks first.
    fn change(&self, f: impl FnOnce(&mut NavState)) {
        let mut nav = self.nav.get_untracked();
        let on_config = config::on_config(&nav);
        f(&mut nav);
        if on_config && self.asks_to_leave(&nav) {
            return;
        }
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
}

fn cached_namespace(feed: &daemon::Feed) -> bool {
    feed.store.is_some()
        && (feed.session.is_some() || !feed.capabilities.is_empty() || feed.board.complete)
}

fn startup(
    nav: &mut NavState,
    pairs: bool,
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
    let data = if feed.browser_pairing_pending() || (pairs && !configured) {
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

    /// A browser without a saved pairing showed Now's skeletons and "corrald is not reachable"
    /// while it dialed nothing: with no paired marker it opens Connection's pairing form at once
    /// and says it is not paired; with the marker it still connects (and may be unreachable).
    #[test]
    fn unpaired_browser_opens_the_pairing_form_and_does_not_call_corrald_unreachable() {
        for form in [Form::Desktop, Form::Phone] {
            let mut nav = NavState::new(form);
            nav.destination = Destination::Home;
            let configured = crate::client::saved_pairing(None);
            let (data, feed) = startup(&mut nav, true, configured, Ok(None));
            assert_eq!(nav.destination, Destination::Settings);
            assert_eq!(nav.settings, SettingsSection::Connection);
            assert_eq!(data, DataState::Disconnected);
            assert!(matches!(
                crate::model::devices::pair_browser(&feed),
                PairBrowser::Form { prefilled: false }
            ));
            for client in [
                crate::model::devices::PairClient::Browser,
                crate::model::devices::PairClient::Native,
            ] {
                let guide = crate::model::settings::connect::pair_guide(&feed, client, None);
                let empty = crate::model::settings::connect::unavailable(&feed, Some(client));
                for line in [guide.status, empty.reason] {
                    assert!(!line.contains("reachable"), "{line}");
                }
            }
            let mut nav = NavState::new(form);
            let (data, _) = startup(
                &mut nav,
                true,
                crate::client::saved_pairing(Some("1")),
                Ok(None),
            );
            assert_eq!(data, DataState::Daemon);
        }
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
