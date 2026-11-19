use corral_protocol::session_state::{Capability, SessionState};

use super::{Cx, Facts, SessionRole, Tab, Ui, tabs_for};
use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::health::daemon_env;
use crate::model::nav::Form;
use crate::model::session::transcript::LiveSession;

fn capability(id: &str) -> Capability {
    Capability {
        id: id.into(),
        unavailable: None,
    }
}

/// The tabs a build's page derives from what its host declares, with every body drawn.
fn build_tabs(can: &[&str]) -> Vec<Tab> {
    let id = Id::from("B-k3x9");
    let mut feed = Feed::default();
    let mut live = LiveSession::default();
    live.replace_state(Some(Box::new(SessionState {
        can: can.iter().map(|id| capability(id)).collect(),
        ..SessionState::default()
    })));
    feed.live.insert(id.clone(), live);
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let ui = Ui::default();
    let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);
    let all = [
        Tab::Task,
        Tab::Chat,
        Tab::Agents,
        Tab::Changes,
        Tab::Board,
        Tab::Proof,
    ];
    super::tabs::items(&cx, &all)
        .into_iter()
        .map(|item| item.tab)
        .collect()
}

/// A build whose host has a board connection gets the Board tab; a bare build, whose host does
/// not declare the board part, has none rather than an empty or failing one.
#[test]
fn a_build_offers_board_and_a_bare_build_does_not() {
    assert_eq!(
        tabs_for(&SessionRole::Build, false, false),
        [Tab::Chat, Tab::Agents, Tab::Changes, Tab::Board]
    );
    assert_eq!(
        tabs_for(&SessionRole::Build, false, true),
        [Tab::Chat, Tab::Agents, Tab::Changes]
    );
    assert_eq!(
        build_tabs(&["send", "children", "changes", "board"]),
        [Tab::Chat, Tab::Agents, Tab::Changes, Tab::Board]
    );
    assert_eq!(
        build_tabs(&["send", "children", "changes"]),
        [Tab::Chat, Tab::Agents, Tab::Changes]
    );
}

/// The role decides Task and Proof: a task-bound session has them, a chat does not.
#[test]
fn only_a_task_bound_session_has_task_and_proof() {
    assert_eq!(
        tabs_for(&SessionRole::Worker, true, true),
        [Tab::Task, Tab::Chat, Tab::Agents, Tab::Changes, Tab::Proof]
    );
    assert_eq!(
        tabs_for(&SessionRole::Assistant, false, true),
        [Tab::Chat, Tab::Agents, Tab::Changes]
    );
}
