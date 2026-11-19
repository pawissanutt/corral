use corral_protocol::detail::CardKind;
use corral_protocol::session_state::SessionState;

use crate::model::Id;
use crate::model::session::questions_tests::{SESSION, card, dock_of, feed_with, option, question};
use crate::model::session::{Action, DockAction, State};

/// A child's question docked in full took more than half the page and covered the tab under it:
/// it is one line until the owner opens it, and Not now folds it back.
#[test]
fn a_waiting_card_is_one_line_until_opened() {
    let feed = feed_with(
        card(
            "c-1",
            CardKind::ChildQuestion,
            vec![question(vec![option("yes"), option("no")], false)],
        ),
        SessionState::default(),
    );
    let session = Id::from(SESSION);
    let mut state = State::default();

    let folded = dock_of(&feed, &state);
    assert!(folded.choice.is_none());
    assert_eq!(folded.waiting.len(), 1);
    assert_eq!(folded.waiting[0].id, "dock-c-1");

    state.act(folded.waiting[0].action.clone(), &feed);
    let opened = dock_of(&feed, &state);
    assert!(opened.choice.is_some());
    assert!(opened.waiting.is_empty());

    state.act(
        Action::Dock(DockAction::Dismiss {
            session,
            card: "c-1".into(),
        }),
        &feed,
    );
    let again = dock_of(&feed, &state);
    assert!(again.choice.is_none());
    assert_eq!(again.waiting.len(), 1);
}
