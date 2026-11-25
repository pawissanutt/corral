//! Device-local dock preferences and the pane's tab labels.

use super::agents::switcher::StoreAgents;
use super::agents::{session_line, switcher};
use super::daemon::Feed;
use super::session::ended_run;
use super::status::SessionState;
use super::{Form, Id};

pub const LIMIT: usize = 6;
pub const FULL: &str = "The side pane is full. Undock a session first.";
pub const NO_STORE: &str = "Cannot dock without an authenticated daemon store.";
pub const UNAVAILABLE: &str = "This session is no longer available to dock.";
pub const OPEN_BESIDE: &str = "Open beside";
pub const DOCK_SIDE: &str = "Dock to side";
pub const OPEN_PAGE: &str = "Open page";
pub const UNDOCK: &str = "Undock";
pub const DOCK_LABEL: &str = "Docked sessions";
pub const CONVERSATION_BESIDE: &str = "The conversation is in the side pane.";

pub fn supported(form: Form) -> bool {
    form == Form::Desktop
}

pub fn exact(feed: &Feed, id: &Id) -> Option<String> {
    switcher::exact(feed, id.as_str()).or_else(|| {
        (feed.agents.contains_key(id.as_str()) || ended_run(feed, id.as_str()).is_some())
            .then(|| id.to_string())
    })
}

impl StoreAgents {
    pub fn show_docked(&mut self, id: &str) -> bool {
        if !self.docked.iter().any(|tab| tab == id) {
            if self.docked.len() >= LIMIT {
                return false;
            }
            self.docked.push(id.to_owned());
        }
        self.select_docked(id)
    }

    pub fn select_docked(&mut self, id: &str) -> bool {
        if !self.docked.iter().any(|tab| tab == id) {
            return false;
        }
        self.visible_dock = Some(id.to_owned());
        true
    }

    pub fn undock(&mut self, id: &str) {
        if let Some(at) = self.docked.iter().position(|tab| tab == id) {
            self.docked.remove(at);
            if self.visible_dock.as_deref() == Some(id) {
                self.visible_dock = self.docked.get(at).or_else(|| self.docked.last()).cloned();
            }
        }
    }

    pub fn visible_dock(&self) -> Option<&str> {
        self.visible_dock
            .as_deref()
            .filter(|id| self.docked.iter().any(|tab| tab == id))
            .or_else(|| self.docked.first().map(String::as_str))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Tab {
    pub id: Id,
    pub label: String,
    pub state: SessionState,
}

pub fn tabs(feed: &Feed, saved: &StoreAgents) -> Vec<Tab> {
    saved
        .docked
        .iter()
        .map(|id| {
            let state = session_line(feed, id).map_or(SessionState::Gone, |line| line.state);
            Tab {
                id: Id::new(id),
                label: format!("{id} · {}", state.word()),
                state,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::agents::Pins;

    #[test]
    fn adding_and_selecting_does_not_duplicate_or_reorder_tabs() {
        let mut saved = StoreAgents::default();
        assert!(saved.show_docked("B-1"));
        assert_eq!(saved.visible_dock(), Some("B-1"));
        assert!(saved.show_docked("T-2_R1"));
        assert_eq!(saved.visible_dock(), Some("T-2_R1"));
        assert!(!saved.select_docked("A-missing"));
        assert!(saved.select_docked("B-1"));
        assert_eq!(saved.visible_dock(), Some("B-1"));
        assert_eq!(saved.docked, ["B-1", "T-2_R1"]);
    }

    #[test]
    fn removing_the_visible_tab_selects_a_neighbor() {
        let mut saved = StoreAgents::default();
        saved.show_docked("A-1");
        saved.show_docked("B-1");
        saved.undock("B-1");
        assert_eq!(saved.visible_dock(), Some("A-1"));
        saved.undock("A-1");
        assert_eq!(saved.visible_dock(), None);
    }

    #[test]
    fn a_full_dock_refuses_new_tabs_but_can_select_existing_ones() {
        let mut saved = StoreAgents::default();
        for index in 0..LIMIT {
            assert!(saved.show_docked(&format!("A-{index}")));
        }
        let before = saved.clone();
        assert!(!saved.show_docked("A-extra"));
        assert_eq!(saved, before);
        assert!(saved.show_docked("A-0"));
        assert_eq!(saved.visible_dock(), Some("A-0"));
    }

    #[test]
    fn old_pins_load_and_docks_round_trip_without_crossing_store_keys() {
        let mut pins = Pins::parse(r#"{"store":{"pinned":["A-1"],"last":"A-2"}}"#).unwrap();
        assert!(pins.pinned("store", "A-1"));
        assert_eq!(pins.get("store").unwrap().visible_dock(), None);
        pins.store_mut("store").show_docked("B-1");
        pins.store_mut("other").show_docked("T-1_R1");
        let saved = Pins::parse(&pins.encode().unwrap()).unwrap();
        assert_eq!(saved, pins);
        assert_eq!(saved.get("store").unwrap().visible_dock(), Some("B-1"));
        assert_eq!(saved.get("other").unwrap().visible_dock(), Some("T-1_R1"));
    }
}
