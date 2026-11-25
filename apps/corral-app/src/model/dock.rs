//! The desktop side pane: Ask's tab, then the sessions this device keeps open beside the page
//! (any session: a chat, a Code or Build session, a worker's, reviewer's or planner's run),
//! with their device-local preferences and the words every entry point shares.

use super::agents::switcher::StoreAgents;
use super::agents::{session_line, switcher};
use super::daemon::Feed;
use super::session::ended_run;
use super::status::SessionState;
use super::{Form, Id};

/// Session tabs the pane holds beside Ask's.
pub const LIMIT: usize = 6;
/// The pane's one name, as its region, the sidebar and the palette say it.
pub const PANE: &str = "Side pane";
pub const FULL: &str = "The side pane holds six sessions. Close one of its tabs first.";
pub const NO_STORE: &str = "Cannot open in the side pane without an authenticated daemon store.";
pub const UNAVAILABLE: &str = "This session is no longer available to open in the side pane.";
/// What puts a session in the pane, from its menu, its page head, the switcher and the palette.
pub const OPEN_BESIDE: &str = "Open in side pane";
/// What takes the session shown on its own page back out of the pane.
pub const MOVE_BACK: &str = "Move back to the page";
pub const OPEN_PAGE: &str = "Open session ↗";
pub const HIDE: &str = "Hide side pane";
pub const SHOW: &str = "Show side pane";
pub const TOGGLE: &str = "Show or hide side pane";
/// The chord that shows or hides the pane, as key caps write it and as it is bound.
pub const KEY: &str = "Ctrl J";
pub const CHORD: &str = "mod+j";
pub const CLOSE_TAB: &str = "Close tab";
pub const ASK_TAB: &str = "Ask";
/// The strip's +, and what it offers.
pub const NEW_TAB: &str = "Open a new session in the side pane";
pub const NEW_CHAT: &str = "New chat";
pub const NEW_CODE: &str = "New Code session…";
pub const NEW_BUILD: &str = "New build session…";
pub const OPEN_OTHER: &str = "Open a conversation…  Ctrl G";
pub const CONVERSATION_BESIDE: &str = "This conversation is open in the side pane.";

pub fn supported(form: Form) -> bool {
    form == Form::Desktop
}

/// The exact id a tab keeps for `id`: Ask's `quick` as the chat it names; a chat, Code or
/// Build session; a run of any role, ended or not; or one child agent's conversation
/// (`<session>/child/<id>`) of such a session.
pub fn exact(feed: &Feed, id: &Id) -> Option<String> {
    if let Some((parent, child)) = id.as_str().split_once(CHILD) {
        if child.is_empty() || child.contains(CHILD) {
            return None;
        }
        let parent = exact(feed, &Id::new(parent))?;
        return Some(format!("{parent}{CHILD}{child}"));
    }
    switcher::exact(feed, id.as_str()).or_else(|| {
        (feed.agents.contains_key(id.as_str()) || ended_run(feed, id.as_str()).is_some())
            .then(|| id.to_string())
    })
}

impl StoreAgents {
    /// Adds `id`'s tab (once) and selects it. A full pane first gives up the oldest tab of a
    /// session that `gone` says has ended; with none, it refuses.
    pub fn show_docked(&mut self, id: &str, gone: impl Fn(&str) -> bool) -> bool {
        if !self.docked.iter().any(|tab| tab == id) {
            if self.docked.len() >= LIMIT {
                let Some(ended) = self.docked.iter().position(|tab| gone(tab)) else {
                    return false;
                };
                self.docked.remove(ended);
            }
            self.docked.push(id.to_owned());
        }
        self.pane_hidden = false;
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
        self.shown_tab(None)
    }

    /// The session tab the pane shows: the one last chosen, else the first; never `ask`, the
    /// chat Ask's own tab shows, so one conversation never has two tabs.
    pub fn shown_tab(&self, ask: Option<&str>) -> Option<&str> {
        let listed = |id: &&str| Some(*id) != ask && self.docked.iter().any(|tab| tab == id);
        self.visible_dock
            .as_deref()
            .filter(listed)
            .or_else(|| self.docked.iter().map(String::as_str).find(listed))
    }
}

const CHILD: &str = "/child/";

/// One session tab: the session's name (its id when it has none), and what a screen reader
/// says of it.
#[derive(Clone, PartialEq, Eq)]
pub struct Tab {
    pub id: Id,
    pub label: String,
    pub spoken: String,
    pub state: SessionState,
}

/// The session tabs in the order they were opened, less `ask`, the chat Ask's tab shows.
pub fn tabs(feed: &Feed, saved: &StoreAgents, ask: Option<&str>) -> Vec<Tab> {
    saved
        .docked
        .iter()
        .filter(|id| Some(id.as_str()) != ask)
        .map(|id| {
            let (name, state) = match id.split_once(CHILD) {
                Some((parent, child)) => child_tab(feed, parent, child),
                None => session_tab(feed, id),
            };
            let label = name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| id.clone());
            Tab {
                id: Id::new(id),
                spoken: format!("{label} · {id} · {}", state.word()),
                label,
                state,
            }
        })
        .collect()
}

fn session_tab(feed: &Feed, id: &str) -> (Option<String>, SessionState) {
    let line = session_line(feed, id);
    let state = line.as_ref().map_or(SessionState::Gone, |line| line.state);
    let named = line
        .map(|line| line.name)
        .filter(|name| !name.trim().is_empty() && name != id);
    (named.or_else(|| unnamed(feed, id)), state)
}

/// A readable name for a session its host did not name: a Code or Build session by its
/// build's name or its checkout's folder and branch, a chat by the task it is about, a run by
/// its task and role. None leaves the id.
fn unnamed(feed: &Feed, id: &str) -> Option<String> {
    if let Some(row) = feed.assistants.get(id) {
        if let Some(build) = &row.build {
            let kind = if build.mode == corral_protocol::build::BuildMode::Code {
                "Code"
            } else {
                "Build"
            };
            let folder = [&build.worktree, &build.repo_root]
                .into_iter()
                .filter_map(|path| path.trim_end_matches('/').rsplit('/').next())
                .find(|folder| !folder.is_empty());
            let name = Some(build.name.trim())
                .filter(|name| !name.is_empty() && *name != id)
                .or(folder)?;
            return Some(match build.branch.as_deref().filter(|b| !b.is_empty()) {
                Some(branch) if branch != name => format!("{kind} · {name} ({branch})"),
                _ => format!("{kind} · {name}"),
            });
        }
        return Some(match row.focus.as_deref().filter(|f| !f.is_empty()) {
            Some(focus) => format!("Chat · {focus}"),
            None => "Chat".to_owned(),
        });
    }
    let (task, run) = id.split_once('_')?;
    let role = feed
        .agents
        .get(id)
        .map(|row| row.run.role.as_str().to_owned());
    Some(match role {
        Some(role) => format!("{task} {run} · {role}"),
        None => format!("{task} {run}"),
    })
}

/// A child agent's tab, from its session's fleet as corrald streams it: its id and title, and
/// its state (asking you is Blocked; queued or running is Working; finished is Gone). Without
/// a fleet read, or in a state this app does not know, the child is as its session is.
fn child_tab(feed: &Feed, parent: &str, child: &str) -> (Option<String>, SessionState) {
    let found = feed
        .live
        .get(parent)
        .and_then(|live| live.fleet.as_ref())
        .and_then(|fleet| fleet.children.iter().find(|c| c.id == child));
    let Some(found) = found else {
        return (Some(child.to_owned()), session_tab(feed, parent).1);
    };
    let state = if found.question == Some(true) || found.state == "needs" {
        SessionState::Blocked
    } else {
        match found.state.as_str() {
            "queued" | "running" => SessionState::Working,
            "done" | "failed" | "cancelled" => SessionState::Gone,
            _ => session_tab(feed, parent).1,
        }
    };
    let title = found.title.trim();
    let name = if title.is_empty() {
        child.to_owned()
    } else {
        format!("{child} · {title}")
    };
    (Some(name), state)
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
        assert!(saved.show_docked("B-1", |_| false));
        assert_eq!(saved.visible_dock(), Some("B-1"));
        assert!(saved.show_docked("T-2_R1", |_| false));
        assert_eq!(saved.visible_dock(), Some("T-2_R1"));
        assert!(!saved.select_docked("A-missing"));
        assert!(saved.select_docked("B-1"));
        assert_eq!(saved.visible_dock(), Some("B-1"));
        assert_eq!(saved.docked, ["B-1", "T-2_R1"]);
    }

    #[test]
    fn removing_the_visible_tab_selects_a_neighbor() {
        let mut saved = StoreAgents::default();
        saved.show_docked("A-1", |_| false);
        saved.show_docked("B-1", |_| false);
        saved.undock("B-1");
        assert_eq!(saved.visible_dock(), Some("A-1"));
        saved.undock("A-1");
        assert_eq!(saved.visible_dock(), None);
    }

    #[test]
    fn a_full_dock_refuses_new_tabs_but_can_select_existing_ones() {
        let mut saved = StoreAgents::default();
        for index in 0..LIMIT {
            assert!(saved.show_docked(&format!("A-{index}"), |_| false));
        }
        let before = saved.clone();
        assert!(!saved.show_docked("A-extra", |_| false));
        assert_eq!(saved, before);
        assert!(saved.show_docked("A-0", |_| false));
        assert_eq!(saved.visible_dock(), Some("A-0"));
        // A finished session's tab gives way, the oldest first, so the pane never fills up
        // with sessions that have ended.
        assert!(saved.show_docked("A-extra", |id| id == "A-2" || id == "A-4"));
        assert_eq!(saved.docked, ["A-0", "A-1", "A-3", "A-4", "A-5", "A-extra"]);
        assert_eq!(saved.visible_dock(), Some("A-extra"));
    }

    #[test]
    fn old_pins_load_and_docks_round_trip_without_crossing_store_keys() {
        let mut pins = Pins::parse(r#"{"store":{"pinned":["A-1"],"last":"A-2"}}"#).unwrap();
        assert!(pins.pinned("store", "A-1"));
        assert_eq!(pins.get("store").unwrap().visible_dock(), None);
        pins.store_mut("store").show_docked("B-1", |_| false);
        pins.store_mut("other").show_docked("T-1_R1", |_| false);
        let saved = Pins::parse(&pins.encode().unwrap()).unwrap();
        assert_eq!(saved, pins);
        assert_eq!(saved.get("store").unwrap().visible_dock(), Some("B-1"));
        assert_eq!(saved.get("other").unwrap().visible_dock(), Some("T-1_R1"));
    }

    #[test]
    fn asks_chat_is_never_the_shown_session_tab() {
        let mut saved = StoreAgents::default();
        saved.show_docked("A-1", |_| false);
        saved.show_docked("B-1", |_| false);
        saved.select_docked("A-1");
        assert_eq!(saved.shown_tab(None), Some("A-1"));
        assert_eq!(saved.shown_tab(Some("A-1")), Some("B-1"));
        saved.undock("B-1");
        assert_eq!(saved.shown_tab(Some("A-1")), None);
    }

    #[test]
    fn a_child_conversation_docks_by_its_sessions_exact_id_one_level_deep() {
        let mut feed = Feed::default();
        let row = crate::model::agents::source_tests::assistant(
            "B-1",
            corral_protocol::rows::AssistantKind::Build,
        );
        feed.assistants.insert("B-1".into(), row);
        assert_eq!(
            exact(&feed, &Id::new("B-1/child/docs")).as_deref(),
            Some("B-1/child/docs")
        );
        for refused in ["B-1/child/", "B-1/child/a/child/b", "B-9/child/docs"] {
            assert_eq!(exact(&feed, &Id::new(refused)), None, "{refused}");
        }
    }
}
