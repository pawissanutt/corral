//! The dock follows the active authenticated store, not the route or the Ask key.

use nsh_core::prelude::*;

use super::{AppState, switcher::pin_key};
use crate::model::agents::switcher::StoreAgents;
use crate::model::{DataState, Id, Overlay, dock, work::Press};

impl AppState {
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
        if self.is_open(Overlay::Ask) {
            return None;
        }
        self.visible_dock()
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
        }) && untracked(|| self.is_open(Overlay::Ask))
        {
            self.close(Overlay::Ask);
        }
    }

    pub fn select_dock(&self, id: &Id) {
        self.change_dock(|saved| {
            saved.select_docked(id.as_str());
            Ok(())
        });
    }

    pub fn undock(&self, id: &Id) {
        self.change_dock(|saved| {
            saved.undock(id.as_str());
            Ok(())
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
