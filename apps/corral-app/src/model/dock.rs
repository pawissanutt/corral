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

pub const CLOSE_PLAN: &str = "Done";
pub const DETAILS: &str = "Details";
pub const ATTACH_PAGE: &str = "Attach what I'm looking at";

/// Full views belong to the page; local actions like transcript expansion do not navigate.
pub fn page_for(action: &super::session::Action) -> Option<super::Live> {
    use super::session::{Action, ChangesAction, FilesAction, PlanAction, SidebarAction};
    let session = match action {
        Action::Tab { session, .. }
        | Action::Sub { session, .. }
        | Action::Changes(ChangesAction::Open { session, .. })
        | Action::Files(FilesAction::Open { session, .. })
        | Action::Sidebar(
            SidebarAction::Agent { session, .. }
            | SidebarAction::Job { session, .. }
            | SidebarAction::Plan(PlanAction::OpenPage { session, .. }),
        ) => session,
        _ => return None,
    };
    Some(super::Live::Session(session.clone()))
}

pub fn details(
    cx: &super::session::Cx<'_>,
    panel: &super::session::Panel,
) -> Option<super::session::Details> {
    use super::session::{Action, PlanAction, SectionContent, SidebarAction};
    let open_step = |step| {
        Action::Sidebar(SidebarAction::Plan(PlanAction::OpenPage {
            session: cx.id.clone(),
            step,
        }))
    };
    let mut details = super::session::details(cx, panel)?;
    for section in &mut details.sections {
        if let Some(SectionContent::Agents(tab)) = &mut section.content {
            for row in &mut tab.agents {
                // A waiting child still opens its full view here; its question
                // remains actionable from the conversation below.
                row.action = Action::Sidebar(SidebarAction::Agent {
                    session: cx.id.clone(),
                    child: row.child.clone(),
                });
            }
        }
        if let Some(SectionContent::Plan(tab)) = &mut section.content {
            tab.open = open_step(None);
            for row in &mut tab.steps {
                row.action = open_step(Some(row.id.clone()));
            }
        }
    }
    Some(details)
}

pub fn page_context(
    live: Option<super::Live>,
    session: &super::session::State,
    artifact: Option<(&str, Option<&str>)>,
) -> Option<corral_protocol::assistant::ContextRef> {
    use super::session::Tab;
    use corral_protocol::assistant::ContextRef;
    if let Some(live) = live {
        if let super::Live::Session(id) = &live {
            let ui = session.ui(id);
            let file = match ui.tab {
                Tab::Changes => ui.changes.file.as_deref(),
                Tab::Files => ui.files.open.as_deref().filter(|_| !ui.files.browsing),
                _ => None,
            };
            if let Some(file) = file {
                return Some(ContextRef {
                    kind: "file".into(),
                    id: format!("{id}/{file}"),
                    rev: None,
                });
            }
        }
        return Some(live.context(None));
    }
    artifact.map(|(reference, section)| ContextRef {
        kind: "artifact".into(),
        id: section.map_or_else(
            || reference.to_owned(),
            |section| format!("{reference}#{section}"),
        ),
        rev: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::agents::Pins;

    #[test]
    fn dock_links_target_the_session_page_but_local_actions_do_not() {
        use crate::model::session::{
            Action, ChangesAction, FilesAction, PlanAction, SidebarAction,
        };
        let session = Id::new("B-1");
        let links = [
            Action::Changes(ChangesAction::Open {
                session: session.clone(),
                path: "guide.md".into(),
            }),
            Action::Files(FilesAction::Open {
                session: session.clone(),
                path: "guide.md".into(),
                directory: false,
            }),
            Action::Sidebar(SidebarAction::Agent {
                session: session.clone(),
                child: "docs".into(),
            }),
            Action::Sidebar(SidebarAction::Job {
                session: session.clone(),
                job: "j1".into(),
            }),
            Action::Sidebar(SidebarAction::Plan(PlanAction::OpenPage {
                session: session.clone(),
                step: Some("S1".into()),
            })),
        ];
        for link in links {
            assert_eq!(
                page_for(&link),
                Some(crate::model::Live::Session(session.clone())),
                "{link:?}"
            );
        }
        assert_eq!(page_for(&Action::Sidebar(SidebarAction::Toggle)), None);
    }

    #[test]
    fn attaching_uses_the_open_file_or_cited_section_not_just_its_parent() {
        use crate::model::{
            Live,
            session::{self, Tab},
        };
        let mut state = session::State::default();
        let session = Id::new("B-1");
        let task = Live::Task(Id::new("T-8"));
        assert_eq!(
            page_context(Some(task.clone()), &state, None),
            Some(task.context(None))
        );
        let live = Live::Session(session.clone());
        assert_eq!(
            page_context(Some(live.clone()), &state, None),
            Some(live.context(None))
        );
        let mut ui = session::Ui {
            tab: Tab::Changes,
            ..Default::default()
        };
        ui.changes.file = Some("docs/guide.md".into());
        state.sessions.insert(session, ui);
        let file = page_context(Some(live), &state, None).unwrap();
        assert_eq!(
            (file.kind.as_str(), file.id.as_str()),
            ("file", "B-1/docs/guide.md")
        );
        let artifact = page_context(None, &state, Some(("T-2:3", Some("goal")))).unwrap();
        assert_eq!(
            (artifact.kind.as_str(), artifact.id.as_str()),
            ("artifact", "T-2:3#goal")
        );
    }

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
