//! The desktop side pane: Ask's tab and the session tabs kept beside the page. The session
//! tabs follow the active authenticated store, not the route or the Ask key; Ask's tab is
//! `Overlay::Ask`. One pane shows one tab at a time, and closing the pane closes both.

use nsh_core::prelude::*;

use super::{AppState, switcher::pin_key};
use crate::model::agents::switcher::StoreAgents;
use crate::model::pane::Placement;
use crate::model::{DataState, Id, Overlay, dock, work::Press};

use std::collections::BTreeMap;

use corral_protocol::assistant::ContextRef;

use crate::model::Destination;
use crate::model::session::{Action, Panel, PlanAction, SidebarAction};

type Contexts = BTreeMap<((bool, String), Id), ContextRef>;

#[derive(Clone, Copy)]
pub(super) struct DockUi {
    contexts: Signal<Contexts>,
    panel: Option<Signal<Panel>>,
    /// Asked for since the window last became too narrow to keep the pane beside the page:
    /// a narrow window shows the pane over the page only on request, never on its own.
    narrow_shown: Signal<bool>,
    /// A narrow window's pane stepped aside for the page a citation in it opened: still held,
    /// and mounted, until it is asked back.
    aside: Signal<bool>,
    /// This copy draws inside the side pane (Ask's tab or a session's): a citation pressed
    /// through it was pressed in the pane, not in the page beside or under it.
    in_pane: bool,
    /// A citation pressed in the pane, between its press and the page it opens being reached.
    cite: Signal<PaneCite>,
}

/// Where a citation pressed in the pane stands on its way to the page it opens.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PaneCite {
    Idle,
    /// Its effects are running; still `Armed` after them, the page went where it asked.
    Armed,
    /// Leaving unsaved Configuration asked first: the pane steps aside only if Discard goes on.
    Deferred,
}

impl DockUi {
    pub(super) fn new() -> Self {
        Self {
            contexts: signal(BTreeMap::new()),
            panel: None,
            narrow_shown: signal(false),
            aside: signal(false),
            in_pane: false,
            cite: signal(PaneCite::Idle),
        }
    }
}

impl AppState {
    /// A mounted pane's copy; never installed in the root AppState or saved.
    pub fn for_dock(mut self) -> Self {
        self.dock_ui.panel = Some(signal(Panel::default()));
        self.for_pane_citations()
    }

    /// The copy the side pane draws with, Ask's tab as a session's: what it marks is that a
    /// citation pressed through it was pressed in the pane. Nothing else about it changes.
    pub fn for_pane_citations(mut self) -> Self {
        self.dock_ui.in_pane = true;
        self
    }

    /// Whether this copy draws inside the side pane.
    pub(super) fn cites_from_pane(&self) -> bool {
        self.dock_ui.in_pane
    }

    /// Runs what a citation pressed in the pane opens. A narrow pane steps aside only once the
    /// page has gone there, never before: leaving unsaved Configuration asks first, and the
    /// pane waits for that answer (stepping aside on Discard, staying on Cancel).
    pub(super) fn open_from_pane(&self, effects: Vec<crate::model::Effect>) {
        self.dock_ui.cite.set(PaneCite::Armed);
        self.run(effects);
        if self.dock_ui.cite.get_untracked() == PaneCite::Armed {
            self.dock_ui.cite.set(PaneCite::Idle);
            self.set_pane_aside();
        }
    }

    /// Opens a newly started session from the pane without closing Ask before the navigation
    /// guard has decided whether Configuration can be left.
    pub(super) fn open_started_from_pane(&self, id: Id) {
        self.dock_ui.cite.set(PaneCite::Armed);
        self.change(|nav| nav.open_live(crate::model::Live::Session(id)));
        if self.dock_ui.cite.get_untracked() == PaneCite::Armed {
            self.dock_ui.cite.set(PaneCite::Idle);
            self.set_pane_aside();
        }
    }

    /// Leaving unsaved Configuration is about to ask first: a pane citation running now waits
    /// for the answer, and any earlier one that waited is forgotten, its question replaced.
    pub(super) fn defer_pane_cite(&self) {
        let next = match self.dock_ui.cite.get_untracked() {
            PaneCite::Armed => PaneCite::Deferred,
            PaneCite::Idle | PaneCite::Deferred => PaneCite::Idle,
        };
        self.dock_ui.cite.set(next);
    }

    /// The confirm sheet's leave went on (Discard), or did not (`left` false): a pane citation
    /// that waited for it steps a narrow pane aside only for the page it reached.
    pub(super) fn settle_pane_cite(&self, left: bool) {
        if self.dock_ui.cite.get_untracked() == PaneCite::Deferred {
            self.dock_ui.cite.set(PaneCite::Idle);
            if left {
                self.set_pane_aside();
            }
        }
    }

    pub fn dock_panel(&self) -> Option<Signal<Panel>> {
        self.dock_ui.panel
    }

    pub fn session_press(&self, action: Action) {
        if let Some(panel) = self.dock_panel() {
            match action {
                Action::Sidebar(SidebarAction::Toggle) => {
                    panel.update(|panel| panel.open = Some(!panel.shown()));
                    return;
                }
                Action::Sidebar(SidebarAction::Fold(section)) => {
                    panel.update(|panel| {
                        if !panel.folded.remove(&section) {
                            panel.folded.insert(section);
                        }
                    });
                    return;
                }
                Action::Sidebar(SidebarAction::Preview) => {
                    panel.update(|panel| panel.preview = !panel.preview);
                    return;
                }
                _ => {}
            }
            if let Some(page) = dock::page_for(&action) {
                self.open_dock_page(page);
            }
        }
        if let Some(crate::model::Live::Session(id)) = dock::page_for(&action)
            && !matches!(action, Action::Sidebar(SidebarAction::Plan(_)))
            && self
                .session
                .with_untracked(|state| state.ui(&id).sidebar.plan.in_page)
        {
            self.session_input(
                Action::Sidebar(SidebarAction::Plan(PlanAction::Close(id))),
                false,
            );
        }
        self.session_input(action, false);
    }

    pub fn current_page_context(&self) -> Option<ContextRef> {
        let live = self.live_page();
        let artifact = (live.is_none() && self.destination() == Destination::Artifacts)
            .then(|| {
                self.artifacts.with(|state| {
                    let reference = state
                        .full
                        .clone()
                        .or_else(|| state.selected.clone())
                        .or_else(|| {
                            self.feed.with(|feed| {
                                crate::model::artifacts::page(state, feed, self.form)
                                    .results
                                    .selected
                            })
                        });
                    reference.map(|reference| (reference, state.section.clone()))
                })
            })
            .flatten();
        self.session.with(|session| {
            dock::page_context(
                live,
                session,
                artifact
                    .as_ref()
                    .map(|(id, section)| (id.as_str(), section.as_deref())),
            )
        })
    }

    pub fn dock_context(&self, id: &Id) -> Option<ContextRef> {
        let source = self.dock_source()?;
        self.dock_ui
            .contexts
            .with(|all| all.get(&(source, id.clone())).cloned())
    }

    pub fn set_dock_context(&self, id: &Id, context: Option<ContextRef>) {
        let Some(source) = untracked(|| self.dock_source()) else {
            return;
        };
        let key = (source, id.clone());
        self.dock_ui.contexts.update(|all| {
            if let Some(context) = context {
                all.insert(key, context);
            } else {
                all.remove(&key);
            }
        });
    }

    fn with_dock<T>(&self, read: impl FnOnce(Option<&StoreAgents>) -> T) -> T {
        let preview = self.data.get() == DataState::Fixture;
        let key = self.feed.with(|feed| pin_key(feed, preview));
        self.pins_in_view()
            .with(|pins| read(key.as_deref().and_then(|key| pins.get(key))))
    }

    /// Tracked: the session tab the pane would show, shown or not.
    pub fn visible_dock(&self) -> Option<Id> {
        if !dock::supported(self.form) {
            return None;
        }
        let ask = self.ask_chat();
        self.with_dock(|saved| {
            saved
                .and_then(|saved| saved.shown_tab(ask.as_deref()))
                .map(Id::new)
        })
    }

    /// Tracked: the chat Ask's tab shows (`quick` as the chat it names), which no session tab
    /// shows again.
    fn ask_chat(&self) -> Option<String> {
        let ask = self.ask_id();
        self.feed
            .with(|feed| crate::model::agents::switcher::exact(feed, ask.as_str()))
    }

    /// Tracked: the session tab the pane shows now: none while Ask's tab is chosen, the pane
    /// is hidden, or a narrow window has not asked for it.
    pub fn shown_dock(&self) -> Option<Id> {
        if self.is_open(Overlay::Ask)
            || self.with_dock(|saved| saved.is_some_and(|s| s.pane_hidden))
        {
            return None;
        }
        if self.pane.placement() == Placement::Over && !self.dock_ui.narrow_shown.get() {
            return None;
        }
        self.visible_dock()
    }

    /// Tracked: the conversation the pane shows now: Ask's chat on Ask's tab, else the session
    /// tab's. A page or view drawing the same conversation gives way to it.
    pub fn pane_conversation(&self) -> Option<Id> {
        if self.is_open(Overlay::Ask) {
            return dock::supported(self.form)
                .then(|| self.ask_chat().map(Id::from))
                .flatten();
        }
        self.shown_dock()
    }

    /// Tracked: whether the pane holds the conversation, on a tab of its own or on Ask's open
    /// one.
    pub fn in_pane(&self, id: &Id) -> bool {
        self.docked(id)
            || (self.is_open(Overlay::Ask)
                && dock::supported(self.form)
                && self.ask_chat().as_deref() == Some(id.as_str()))
    }

    /// Takes the conversation out of the pane: Ask's tab closes the pane, a session tab closes.
    pub fn move_back(&self, id: &Id) {
        if untracked(|| {
            self.is_open(Overlay::Ask) && self.ask_chat().as_deref() == Some(id.as_str())
        }) {
            self.close(Overlay::Ask);
        } else {
            self.undock(id);
        }
    }

    /// Tracked: whether the side pane holds a tab to show, over the page or set aside.
    pub fn pane_held(&self) -> bool {
        self.is_open(Overlay::Ask) || self.shown_dock().is_some()
    }

    /// Tracked: whether a narrow window's pane stepped aside for the page a citation in it
    /// opened; it keeps its tab, transcript, scroll and draft for when it comes back.
    pub fn pane_aside(&self) -> bool {
        self.dock_ui.aside.get() && self.pane.placement() == Placement::Over && self.pane_held()
    }

    /// Tracked: whether the side pane shows, on Ask's tab or a session's.
    pub fn pane_open(&self) -> bool {
        self.pane_held() && !self.pane_aside()
    }

    /// Tracked: whether the keys what this copy draws binds may be bound now. A pane set aside
    /// stays mounted, hidden, and its keys (Alt+Enter's send, the `/` menu's and pickers' ↑ ↓,
    /// a card's options) are the window's: while it is aside they are dropped, not just idle,
    /// so they neither act on what it hides nor shadow the page's own keys. Outside the pane,
    /// always.
    pub fn binds_keys(&self) -> bool {
        !self.dock_ui.in_pane || !self.pane_aside()
    }

    /// Binds what `bind` binds while [`Self::binds_keys`] holds, and again each time the pane
    /// comes back; dropped meanwhile.
    pub fn bind_while_shown(self, bind: impl Fn() + 'static) {
        effect(move || {
            if self.binds_keys() {
                untracked(&bind);
            }
        });
    }

    /// A field this copy draws gives the keyboard back while the pane is set aside, and does
    /// not take it again (Tab, a pending focus) until the pane comes back. What it holds stays,
    /// for when it does; its Enter is guarded with [`Self::field_acts`].
    pub fn release_while_aside(self, field: ElementRef) {
        effect(move || {
            if !self.binds_keys() {
                field.blur();
            }
        });
        field.on_focus_in(move || {
            if !self.field_acts() {
                field.blur();
            }
        });
    }

    /// Untracked: whether Enter in a field this copy draws may act now: not while the pane it
    /// is drawn in is set aside, hidden over the page.
    pub fn field_acts(&self) -> bool {
        untracked(|| self.binds_keys())
    }

    /// Untracked: the newest overlay standing over the page. Ask's pane set aside for a narrow
    /// page is held, not over it: Escape and the page's keys are the page's.
    pub(crate) fn overlay_over_page(&self) -> Option<Overlay> {
        if untracked(|| self.pane_aside()) {
            self.nav.with_untracked(|nav| nav.newest_but(Overlay::Ask))
        } else {
            self.nav.with_untracked(crate::model::NavState::newest)
        }
    }

    /// What a citation in the pane opened shows, once the page has gone there: beside a wide
    /// pane, which stays; under a narrow one, which steps aside rather than closing, so nothing
    /// is lost or closed unasked. Only [`Self::open_from_pane`] and [`Self::settle_pane_cite`]
    /// call it, for a press in the pane that reached its page.
    fn set_pane_aside(&self) {
        if untracked(|| self.pane.placement() == Placement::Over && self.pane_open()) {
            self.dock_ui.aside.set(true);
            self.pane.focused.set(false);
        }
    }

    /// Brings a pane that stepped aside back over the page, as it was left.
    pub fn bring_pane_back(&self) {
        if self.dock_ui.aside.get_untracked() {
            self.dock_ui.aside.set(false);
        }
    }

    /// Shows the pane on the session tab last chosen.
    pub fn show_dock(&self) {
        self.set_pane_hidden(false);
        self.dock_ui.narrow_shown.set(true);
        self.bring_pane_back();
    }

    /// Hides the session tabs without closing them; they come back as they were.
    pub fn hide_dock(&self) {
        self.set_pane_hidden(true);
        self.dock_ui.narrow_shown.set(false);
        self.pane.focused.set(false);
        self.bring_pane_back();
    }

    /// Closes the pane, whichever tab it shows.
    pub fn close_pane(&self) {
        if untracked(|| self.is_open(Overlay::Ask)) {
            self.close(Overlay::Ask);
        } else {
            self.hide_dock();
        }
    }

    /// The sidebar's Side pane row, the palette's and `Ctrl J`: closes the pane while it
    /// shows, brings it back when it stepped aside, else shows it on its last session tab, or
    /// on Ask's when it holds none.
    pub fn toggle_pane(&self) {
        if !dock::supported(self.form) {
            return;
        }
        if untracked(|| self.pane_aside()) {
            self.bring_pane_back();
        } else if untracked(|| self.pane_open()) {
            self.close_pane();
        } else if untracked(|| self.visible_dock()).is_some() {
            self.show_dock();
        } else {
            self.open_ask();
        }
    }

    /// Quietly: Ask's tab closes the pane without a store to keep the choice in.
    fn set_pane_hidden(&self, hidden: bool) {
        let preview = self.data.get_untracked() == DataState::Fixture;
        let Some(key) = self.feed.with_untracked(|feed| pin_key(feed, preview)) else {
            return;
        };
        let pins = untracked(|| self.pins_in_view());
        if pins.with_untracked(|pins| pins.get(&key).is_some_and(|s| s.pane_hidden) != hidden) {
            pins.update(|pins| pins.store_mut(&key).pane_hidden = hidden);
        }
    }

    pub(super) fn dock_overlays_page(&self) -> bool {
        self.pane.placement() == Placement::Over
            && self.shown_dock().is_some()
            && !self.pane_aside()
    }

    /// Takes the pane off a narrow page without hiding it from a wide one.
    pub(super) fn dismiss_dock_overlay(&self) -> bool {
        if untracked(|| self.dock_overlays_page()) {
            self.dock_ui.narrow_shown.set(false);
            self.pane.focused.set(false);
            true
        } else {
            false
        }
    }

    /// Escape with the keyboard in the pane's session tab: the pane closes, as Ask's does.
    pub(super) fn escape_pane(&self) -> bool {
        if self.pane.focused.get_untracked() && untracked(|| self.shown_dock()).is_some() {
            self.hide_dock();
            true
        } else {
            false
        }
    }

    pub fn open_dock_page(&self, page: crate::model::Live) {
        self.open_live(page);
        self.dismiss_dock_overlay();
    }

    /// Distinguishes identical session IDs belonging to different sources when mounting a watch.
    pub fn dock_source(&self) -> Option<(bool, String)> {
        let preview = self.data.get() == DataState::Fixture;
        self.feed
            .with(|feed| pin_key(feed, preview))
            .map(|key| (preview, key))
    }

    pub fn dock_tabs(&self) -> Vec<dock::Tab> {
        let ask = self.ask_chat();
        self.with_dock(|saved| {
            saved.map_or_else(Vec::new, |saved| {
                self.feed
                    .with(|feed| dock::tabs(feed, saved, ask.as_deref()))
            })
        })
    }

    /// Tracked: whether the session has a tab in the pane.
    fn docked(&self, id: &Id) -> bool {
        dock::supported(self.form)
            && self.ask_chat().as_deref() != Some(id.as_str())
            && self.with_dock(|saved| {
                saved.is_some_and(|saved| saved.docked.iter().any(|tab| tab == id.as_str()))
            })
    }

    fn change_dock(
        &self,
        change: impl FnOnce(&mut StoreAgents) -> Result<(), &'static str>,
    ) -> bool {
        if !dock::supported(self.form) {
            return false;
        }
        let preview = self.data.get_untracked() == DataState::Fixture;
        let Some(key) = self.feed.with_untracked(|feed| pin_key(feed, preview)) else {
            self.toast(dock::NO_STORE);
            return false;
        };
        let mut result = Ok(());
        untracked(|| self.pins_in_view()).update(|pins| result = change(pins.store_mut(&key)));
        if let Err(why) = result {
            self.toast(why);
            return false;
        }
        true
    }

    /// Puts any session (a chat, a Code or Build session, a run of any role) in the pane on a
    /// tab of its own and shows it there.
    pub fn dock_session(&self, id: &Id) {
        self.try_dock_session(id);
    }

    pub(super) fn try_dock_session(&self, id: &Id) -> bool {
        // A full pane gives up an ended session's tab, or one of Ask's chat (which the strip
        // does not show), first.
        let mut gone: Vec<String> = untracked(|| self.dock_tabs())
            .into_iter()
            .filter(|tab| tab.state == crate::model::status::SessionState::Gone)
            .map(|tab| tab.id.to_string())
            .collect();
        gone.extend(untracked(|| self.ask_chat()));
        let exact = self.feed.with_untracked(|feed| dock::exact(feed, id));
        // Ask's chat already has Ask's tab: show that rather than a second tab of it.
        if exact.is_some() && exact == untracked(|| self.ask_chat()) {
            if dock::supported(self.form) {
                self.open_ask();
            }
            return true;
        }
        if self.change_dock(|saved| {
            let id = exact.ok_or(dock::UNAVAILABLE)?;
            saved
                .show_docked(&id, |tab| gone.iter().any(|gone| gone == tab))
                .then_some(())
                .ok_or(dock::FULL)
        }) {
            self.leave_ask_for_tab();
            self.show_dock();
            true
        } else {
            false
        }
    }

    /// Chooses a session tab of the pane, leaving Ask's tab as it was.
    pub fn select_dock(&self, id: &Id) {
        if self.change_dock(|saved| {
            saved.select_docked(id.as_str());
            Ok(())
        }) {
            self.leave_ask_for_tab();
            self.show_dock();
        }
    }

    /// Closes a session's tab; the session itself goes on.
    pub fn undock(&self, id: &Id) {
        if self.change_dock(|saved| {
            saved.undock(id.as_str());
            Ok(())
        }) {
            self.set_dock_context(id, None);
        }
    }

    /// Ask's tab gives way to a session tab without closing the pane, keeping what Ask's
    /// draft was written about for when its tab is chosen again.
    fn leave_ask_for_tab(&self) {
        if untracked(|| self.is_open(Overlay::Ask)) {
            self.change(|nav| nav.close(Overlay::Ask));
            self.close_attach_field();
        }
    }

    /// The window grew narrow: the pane leaves the page until it is asked for again. Grown wide,
    /// a pane that stepped aside shows beside the page again.
    pub(super) fn follow_dock(self) {
        let over = computed(move || self.pane.placement() == Placement::Over);
        effect(move || {
            let over = over.get();
            untracked(|| {
                self.bring_pane_back();
                if over {
                    self.dock_ui.narrow_shown.set(false);
                }
            });
        });
    }

    pub fn press_work(&self, press: Press) {
        if let Press::OpenBeside(id) = &press {
            self.dock_session(id);
        }
        self.reduce(self.work, |state, feed, _, env| {
            state.press(press, feed, env)
        });
    }
}
