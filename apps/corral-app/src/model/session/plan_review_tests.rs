use corral_protocol::detail::CardKind;
use corral_protocol::session_state::SessionState;

use super::questions_tests::{SESSION, answers, card, feed_with, option, question};
use super::{Action, Body, DockAction, State};
use crate::model::Id;
use crate::model::health::daemon_env;
use crate::model::nav::Form;

/// A briefed plan review must open the plan view, never the generic choice card (which would
/// send Keep planning with no way to say what should change), and Keep planning must carry the
/// owner's feedback under the `revise` option.
#[test]
fn a_plan_review_opens_the_plan_view_and_keep_planning_carries_the_feedback() {
    let options = vec![option("execute"), option("execute_clean"), option("revise")];
    let feed = feed_with(
        card(
            "plan-2-3fa9",
            CardKind::PlanReview,
            vec![question(options, true)],
        ),
        SessionState::default(),
    );
    let session = Id::from(SESSION);
    let mut state = State::default();
    let dock = |action: fn(Id, String) -> DockAction| {
        Action::Dock(action(Id::from(SESSION), "plan-2-3fa9".into()))
    };

    state.act(
        dock(|session, card| DockAction::Bring { session, card }),
        &feed,
    );
    let page = super::page(&feed, &state, &session, daemon_env(&feed, Form::Desktop));
    assert!(matches!(page.body, Body::PlanReview(_)));
    assert!(page.dock.choice.is_none() && page.dock.plan.is_some());

    let asked = state.act(
        dock(|session, card| DockAction::Choose {
            session,
            card,
            option: "revise".into(),
        }),
        &feed,
    );
    assert!(answers(&asked).is_empty());
    state.act(
        dock(|session, card| DockAction::Text {
            session,
            card,
            question: 0,
            text: "split step 2".into(),
        }),
        &feed,
    );
    let sent = answers(&state.act(
        dock(|session, card| DockAction::Submit { session, card }),
        &feed,
    ));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].answers[0].options, ["revise"]);
    assert_eq!(sent[0].answers[0].text.as_deref(), Some("split step 2"));
}
