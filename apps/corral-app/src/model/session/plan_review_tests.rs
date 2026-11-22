use corral_protocol::detail::{CardKind, DetailBody, PlanModeDocPage};
use corral_protocol::session_state::SessionState;

use super::plan_review::target;
use super::questions_tests::{SESSION, answers, card, feed_with, option, question};
use super::{Action, Body, DockAction, State};
use crate::model::Id;
use crate::model::daemon::{ClientError, DetailCache, Feed, detail_key};
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

fn cached(card: &str, body: Option<&str>, error: Option<ClientError>) -> DetailCache {
    let merged = body.map(|body| {
        DetailBody::SessionPlanModeDoc(PlanModeDocPage {
            title: "Plan".into(),
            revision: 1,
            body: body.into(),
            bytes: body.len() as u64,
            offset: 0,
        })
    });
    DetailCache {
        transcript_seams: Default::default(),
        target: target(SESSION, card),
        pages: Vec::new(),
        loading: false,
        stale: false,
        error,
        merged,
    }
}

/// An execute answer runs the whole plan: while only the card's clipped start is shown (the
/// document's read pending, or failed), Execute must not go out by any route, and Keep planning
/// still must.
#[test]
fn execute_waits_for_the_whole_plan_while_keep_planning_does_not() {
    const CARD: &str = "plan-3-77aa01bc";
    let mut clipped = card(
        CARD,
        CardKind::PlanReview,
        vec![question(vec![option("execute"), option("revise")], true)],
    );
    clipped.body = Some("the plan's start".into());
    clipped.body_more = true;
    let mut feed = feed_with(clipped, SessionState::default());
    let session = Id::from(SESSION);
    let choose = |state: &mut State, feed: &Feed, option: &str| {
        answers(&state.act(
            Action::Dock(DockAction::Choose {
                session: Id::from(SESSION),
                card: CARD.into(),
                option: option.into(),
            }),
            feed,
        ))
    };
    let opened = || {
        let mut state = State::default();
        state.act(
            Action::Dock(DockAction::Bring {
                session: Id::from(SESSION),
                card: CARD.into(),
            }),
            &feed,
        );
        state
    };
    let offered = |state: &State, feed: &Feed| {
        let page = super::page(feed, state, &session, daemon_env(feed, Form::Desktop));
        let options = page.dock.plan.map(|plan| plan.options).unwrap_or_default();
        options
            .into_iter()
            .map(|option| option.unavailable.is_some())
            .collect::<Vec<_>>()
    };

    let mut state = opened();
    assert_eq!(offered(&state, &feed), [true, false]);
    assert!(choose(&mut state, &feed, "execute").is_empty());

    let mut revising = opened();
    assert!(choose(&mut revising, &feed, "revise").is_empty());
    let sent = choose(&mut revising, &feed, "revise");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].answers[0].options, ["revise"]);

    let refused = ClientError {
        code: "not_found".into(),
        message: String::new(),
    };
    let failed = cached(CARD, None, Some(refused));
    feed.details.insert(detail_key(&failed.target), failed);
    assert_eq!(offered(&state, &feed), [true, false]);
    assert!(choose(&mut state, &feed, "execute").is_empty());

    let read = cached(CARD, Some("the plan's start and its rest"), None);
    feed.details.insert(detail_key(&read.target), read);
    assert_eq!(offered(&state, &feed), [false, false]);
    let sent = choose(&mut state, &feed, "execute");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].card, CARD);
    assert_eq!(sent[0].answers[0].options, ["execute"]);
}

/// The owner approves what the view shows: a new submission's review must never draw the
/// document read for an earlier one, neither while its own read is pending nor after it failed.
#[test]
fn a_new_submission_never_shows_the_previous_submissions_document() {
    let mut second = card(
        "plan-2-9f3a1c0e",
        CardKind::PlanReview,
        vec![question(vec![option("execute"), option("revise")], true)],
    );
    second.body = Some("second plan".into());
    let mut feed = feed_with(second, SessionState::default());
    let first = cached("plan-1-51c2d7aa", Some("first plan"), None);
    feed.details.insert(detail_key(&first.target), first);
    let session = Id::from(SESSION);
    let mut state = State::default();
    state.act(
        Action::Dock(DockAction::Bring {
            session: session.clone(),
            card: "plan-2-9f3a1c0e".into(),
        }),
        &feed,
    );
    let doc = |feed: &Feed| {
        let page = super::page(feed, &state, &session, daemon_env(feed, Form::Desktop));
        match page.body {
            Body::PlanReview(doc) => doc,
            other => panic!("the plan view is open, not {other:?}"),
        }
    };

    let pending = doc(&feed);
    assert_eq!(pending.body.as_deref(), Some("second plan"));
    assert!(pending.note.is_some() && pending.retry.is_none());

    let refused = ClientError {
        code: "not_found".into(),
        message: String::new(),
    };
    let failed = cached("plan-2-9f3a1c0e", None, Some(refused));
    feed.details.insert(detail_key(&failed.target), failed);
    let failed = doc(&feed);
    assert_eq!(failed.body.as_deref(), Some("second plan"));
    assert_eq!(
        failed.retry,
        Some(Action::Refresh(target(SESSION, "plan-2-9f3a1c0e")))
    );
}
