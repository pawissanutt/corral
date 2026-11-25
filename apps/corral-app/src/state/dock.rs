//! The dock follows the active authenticated store, not the route or the Ask key.

use nsh_core::prelude::*;

use super::{AppState, switcher::pin_key};
use crate::model::agents::switcher::StoreAgents;
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
    hidden: Signal<bool>,
}

impl DockUi {
    pub(super) fn new() -> Self {
        Self {
            contexts: signal(BTreeMap::new()),
            panel: None,
            hidden: signal(false),
        }
    }
}

impl AppState {
    /// A mounted pane's copy; never installed in the root AppState or saved.
    pub fn for_dock(mut self) -> Self {
        self.dock_ui.panel = Some(signal(Panel::default()));
        self
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

    pub fn visible_dock(&self) -> Option<Id> {
        if !dock::supported(self.form) {
            return None;
        }
        self.with_dock(|saved| saved.and_then(StoreAgents::visible_dock).map(Id::new))
    }

    pub fn shown_dock(&self) -> Option<Id> {
        if self.dock_ui.hidden.get() || self.is_open(Overlay::Ask) {
            return None;
        }
        self.visible_dock()
    }

    pub fn can_show_dock(&self) -> bool {
        self.dock_ui.hidden.get() && self.visible_dock().is_some() && !self.is_open(Overlay::Ask)
    }

    pub fn show_dock(&self) {
        self.dock_ui.hidden.set(false);
    }

    pub fn hide_dock(&self) {
        self.dock_ui.hidden.set(true);
        self.pane.focused.set(false);
    }

    pub(super) fn dock_overlays_page(&self) -> bool {
        self.pane.placement() == crate::model::pane::Placement::Over && self.shown_dock().is_some()
    }

    pub(super) fn dismiss_dock_overlay(&self) -> bool {
        if untracked(|| self.dock_overlays_page()) {
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
        self.with_dock(|saved| {
            saved.map_or_else(Vec::new, |saved| {
                self.feed.with(|feed| dock::tabs(feed, saved))
            })
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

    pub fn dock_session(&self, id: &Id) {
        let exact = self.feed.with_untracked(|feed| dock::exact(feed, id));
        if self.change_dock(|saved| {
            let id = exact.ok_or(dock::UNAVAILABLE)?;
            saved.show_docked(&id).then_some(()).ok_or(dock::FULL)
        }) {
            self.show_dock();
            if untracked(|| self.is_open(Overlay::Ask)) {
                self.close(Overlay::Ask);
            }
        }
    }

    pub fn select_dock(&self, id: &Id) {
        self.change_dock(|saved| {
            saved.select_docked(id.as_str());
            Ok(())
        });
    }

    pub fn undock(&self, id: &Id) {
        if self.change_dock(|saved| {
            saved.undock(id.as_str());
            Ok(())
        }) {
            self.set_dock_context(id, None);
        }
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
