use corral_protocol::session_state::{Capability, SessionState};

use super::{Action, Cx, DetailKind, Facts, SessionRole, SidebarAction, State, Ui, details};
use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::health::daemon_env;
use crate::model::nav::Form;
use crate::model::session::transcript::LiveSession;

const SESSION: &str = "B-k3x9";

/// A build whose host serves `can`.
fn feed(can: &[&str]) -> Feed {
    let mut feed = Feed::default();
    let mut session = LiveSession::default();
    session.replace_state(Some(Box::new(SessionState {
        can: can
            .iter()
            .map(|id| Capability {
                id: (*id).into(),
                unavailable: None,
            })
            .collect(),
        ..SessionState::default()
    })));
    feed.live.insert(Id::from(SESSION), session);
    feed
}

fn shown(feed: &Feed, state: &State) -> Option<(bool, Vec<DetailKind>)> {
    let id = Id::from(SESSION);
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let ui = Ui::default();
    let cx = Cx::new(feed, &ui, &id, daemon_env(feed, Form::Desktop), &facts);
    details(&cx, &state.panel).map(|details| {
        (
            details.shown,
            details
                .sections
                .iter()
                .map(|section| section.section)
                .collect(),
        )
    })
}

/// Details opens by default only where it fits beside the conversation, and once the owner
/// closed it a wide page does not open it again; a section the host declared it does not serve
/// is not listed, a board the host serves is.
#[test]
fn details_follow_the_width_until_the_owner_chooses() {
    let mut state = State::default();
    let served = feed(&["changes", "children", "plan", "context", "board"]);
    let bare = feed(&["changes", "children"]);
    let (open, sections) = shown(&served, &state).expect("a build has Details");
    assert!(!open, "closed while the page is narrow");
    assert_eq!(
        sections,
        [
            DetailKind::Changes,
            DetailKind::Agents,
            DetailKind::Plan,
            DetailKind::Usage,
            DetailKind::Context,
            DetailKind::Board
        ]
    );
    assert_eq!(
        shown(&bare, &state).map(|(_, sections)| sections),
        Some(vec![
            DetailKind::Changes,
            DetailKind::Agents,
            DetailKind::Usage
        ])
    );

    state.act(Action::Sidebar(SidebarAction::Wide(true)), &served);
    assert_eq!(shown(&served, &state).map(|(open, _)| open), Some(true));
    state.act(Action::Sidebar(SidebarAction::Toggle), &served);
    assert_eq!(shown(&served, &state).map(|(open, _)| open), Some(false));
    state.act(Action::Sidebar(SidebarAction::Wide(false)), &served);
    state.act(Action::Sidebar(SidebarAction::Wide(true)), &served);
    assert_eq!(shown(&served, &state).map(|(open, _)| open), Some(false));
}
