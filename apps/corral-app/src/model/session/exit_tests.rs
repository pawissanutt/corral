use corral_protocol::detail::{CardKind, CardOption};
use corral_protocol::session_state::{SendPolicy, SessionState};

use super::questions_tests::{SESSION, answers, card, feed_with, option, question};
use super::{Action, DockAction, State};
use crate::model::Id;

/// Land over open findings needs the owner's reason, and it is destructive: once confirmed it
/// must still wait for the reason, and an empty reason must not land.
#[test]
fn a_land_that_needs_a_reason_is_not_sent_without_one() {
    let land = CardOption {
        needs_text: Some("1 major finding is open".into()),
        destructive: true,
        ..option("land")
    };
    let feed = feed_with(
        card(
            "exit-1",
            CardKind::Exit,
            vec![question(
                vec![option("keep"), land, option("iterate")],
                false,
            )],
        ),
        SessionState::default(),
    );
    let session = Id::from(SESSION);
    let mut state = State::default();
    let send = |text: &str| Action::Send {
        session: session.clone(),
        text: text.into(),
        policy: SendPolicy::Steer,
        attachments: Vec::new(),
    };

    let chose = state.act(
        Action::Dock(DockAction::Choose {
            session: session.clone(),
            card: "exit-1".into(),
            option: "land".into(),
        }),
        &feed,
    );
    assert!(answers(&chose).is_empty());
    let confirmed = state.act(
        Action::Dock(DockAction::Confirm {
            session: session.clone(),
            yes: true,
        }),
        &feed,
    );
    assert!(answers(&confirmed).is_empty());
    assert!(answers(&state.act(send("   "), &feed)).is_empty());

    let sent = answers(&state.act(send("the finding is a false positive"), &feed));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].card, "exit-1");
    assert_eq!(sent[0].answers[0].options, ["land"]);
    assert_eq!(
        sent[0].answers[0].text.as_deref(),
        Some("the finding is a false positive")
    );
}
