use corral_protocol::session_state::{Capability, Counts, PlanBrief, SessionState};

use super::{Action, Cx, Facts, SessionRole, SideTab, SidebarAction, State, Ui, sidebar};
use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::health::daemon_env;
use crate::model::nav::Form;
use crate::model::session::transcript::LiveSession;

const SESSION: &str = "B-k3x9";

/// A build whose host serves every sidebar tab and has a plan of three steps, with `live`
/// agents at work.
fn feed(live: u32) -> Feed {
    let mut feed = Feed::default();
    let mut session = LiveSession::default();
    session.replace_state(Some(Box::new(SessionState {
        can: ["context", "changes", "plan"]
            .iter()
            .map(|id| Capability {
                id: (*id).into(),
                unavailable: None,
            })
            .collect(),
        children: Counts {
            live,
            waiting: 0,
            total: live,
        },
        plan: Some(PlanBrief {
            done: 1,
            total: 3,
            current: None,
            stale: false,
        }),
        ..SessionState::default()
    })));
    feed.live.insert(Id::from(SESSION), session);
    feed
}

fn shown(feed: &Feed, ui: &Ui) -> SideTab {
    let id = Id::from(SESSION);
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let cx = Cx::new(feed, ui, &id, daemon_env(feed, Form::Desktop), &facts);
    sidebar(&cx).map(|sidebar| sidebar.tab).unwrap_or_default()
}

/// Until the owner picks, the sidebar follows what became relevant (Plan while nothing works,
/// Context once an agent starts); after the owner picks Files, neither a plan nor a starting
/// agent moves it away again.
#[test]
fn the_auto_tab_never_moves_a_tab_the_owner_picked() {
    let mut state = State::default();
    let idle = feed(0);
    let busy = feed(2);
    assert_eq!(shown(&idle, state.ui(SESSION)), SideTab::Plan);
    assert_eq!(shown(&busy, state.ui(SESSION)), SideTab::Context);

    state.act(
        Action::Sidebar(SidebarAction::Pick {
            session: Id::from(SESSION),
            tab: SideTab::Files,
        }),
        &idle,
    );
    assert_eq!(shown(&idle, state.ui(SESSION)), SideTab::Files);
    assert_eq!(shown(&busy, state.ui(SESSION)), SideTab::Files);
}
