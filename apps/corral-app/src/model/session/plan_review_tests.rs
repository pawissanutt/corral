use corral_protocol::detail::{CardKind, DetailBody, PlanModeDocPage};
use corral_protocol::session_host::SessionCardAnswered;
use corral_protocol::session_state::SessionState;

use super::plan_review::target;
use super::questions_tests::{SESSION, answers, card, feed_with, option, question};
use super::transcript::LiveSession;
use super::{Action, Body, DockAction, State, Tab};
use crate::model::daemon::{ClientError, DetailCache, Feed, Operation, detail_key};
use crate::model::effect::Effect;
use crate::model::health::daemon_env;
use crate::model::nav::Form;
use crate::model::{Id, Live};

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

/// The owner approves the plan they are shown: Ask, a Home card and the inspector draw no plan
/// document, and neither does another view of the session's page, so none of them may send
/// Execute; a review reached there (its line, Alt+↓) must put the document on the page first.
#[test]
fn execute_goes_out_only_from_the_page_showing_the_plan_document() {
    const CARD: &str = "plan-4-0c5e88d1";
    let mut whole = card(
        CARD,
        CardKind::PlanReview,
        vec![question(vec![option("execute"), option("revise")], true)],
    );
    whole.body = Some("the whole plan".into());
    let feed = feed_with(whole, SessionState::default());
    let session = Id::from(SESSION);
    let env = daemon_env(&feed, Form::Desktop);
    let execute = |state: &mut State| {
        answers(&state.act(
            Action::Dock(DockAction::Choose {
                session: Id::from(SESSION),
                card: CARD.into(),
                option: "execute".into(),
            }),
            &feed,
        ))
    };
    let documented = |state: &State| {
        matches!(
            super::page(&feed, state, &session, env).body,
            Body::PlanReview(_)
        )
    };

    // The review is open on the session's page; a compact form of it shows the line only.
    let mut state = State::default();
    let bring = Action::Dock(DockAction::Bring {
        session: session.clone(),
        card: CARD.into(),
    });
    state.act(bring, &feed);
    let compact = super::compact(&feed, &state, &session, env);
    assert!(matches!(compact.body, Body::Chat(_)) && compact.dock.plan.is_none());
    let [line] = compact.dock.waiting.as_slice() else {
        panic!("the review is one line, not {:?}", compact.dock.waiting);
    };
    let mut state = State::default();
    let effects = state.act(line.action.clone(), &feed);
    assert!(answers(&effects).is_empty());
    assert!(effects.contains(&Effect::OpenLive(Live::Session(session.clone()))));
    assert!(documented(&state));

    // Another view of the page: the dock keeps the card, Execute cannot be chosen.
    let tab = Action::Tab {
        session: session.clone(),
        tab: Tab::Agents,
    };
    state.act(tab, &feed);
    assert!(!documented(&state));
    let page = super::page(&feed, &state, &session, env);
    let offered = page.dock.plan.map(|plan| plan.options).unwrap_or_default();
    assert!(offered[0].unavailable.is_some() && offered[1].unavailable.is_none());
    assert!(execute(&mut state).is_empty());

    // Alt+↓ from that view, the review folded: the document takes the main view.
    let mut state = State::default();
    state.act(
        Action::Tab {
            session: session.clone(),
            tab: Tab::Agents,
        },
        &feed,
    );
    state.act(Action::Dock(DockAction::Next(session.clone())), &feed);
    assert!(documented(&state));
    let sent = execute(&mut state);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].card, CARD);
    assert_eq!(sent[0].answers[0].options, ["execute"]);
}

/// The host says `started` before it carries an execute out, and one that fails afterwards
/// leaves the same card waiting: the note typed for the executor must still be in the field
/// then, and be forgotten only once the host no longer briefs the card.
#[test]
fn a_started_execute_keeps_the_note_until_the_review_is_gone() {
    const CARD: &str = "plan-5-2b7d9e40";
    const NOTE: &str = "mind the migration";
    let mut whole = card(
        CARD,
        CardKind::PlanReview,
        vec![question(vec![option("execute"), option("revise")], true)],
    );
    whole.body = Some("the whole plan".into());
    let mut feed = feed_with(whole, SessionState::default());
    let session = Id::from(SESSION);
    let mut state = State::default();
    state.act(
        Action::Dock(DockAction::Bring {
            session: session.clone(),
            card: CARD.into(),
        }),
        &feed,
    );
    state.act(
        Action::Dock(DockAction::Text {
            session: session.clone(),
            card: CARD.into(),
            question: 0,
            text: NOTE.into(),
        }),
        &feed,
    );
    let effects = state.act(
        Action::Dock(DockAction::Choose {
            session: session.clone(),
            card: CARD.into(),
            option: "execute".into(),
        }),
        &feed,
    );
    let Some(Effect::SessionCardAnswer { key, answer }) = effects.first() else {
        panic!("Execute sent no answer: {effects:?}");
    };
    assert_eq!(answer.answers[0].text.as_deref(), Some(NOTE));
    let started = SessionCardAnswered {
        status: "started".into(),
    };
    feed.operations.insert(
        key.clone(),
        Operation {
            method: "session.card.answer".into(),
            loading: false,
            value: serde_json::to_value(started).ok(),
            error: None,
            refusal: None,
        },
    );
    let note = |state: &State| {
        let draft = state.ui(SESSION).dock.drafts.get(CARD);
        draft.map(|draft| draft.text(0).to_owned())
    };

    state.settle(&feed);
    assert_eq!(note(&state).as_deref(), Some(NOTE));

    let mut closed = LiveSession::default();
    closed.replace_state(Some(Box::new(SessionState::default())));
    feed.live.insert(session, closed);
    state.settle(&feed);
    assert_eq!(note(&state), None);
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

/// Behind three older docked cards the plan review was only counted in `+1 more`, with no line
/// to press: the plan could not be read until unrelated cards were answered.
#[test]
fn a_plan_review_keeps_its_line_behind_three_older_cards() {
    let mut feed = feed_with(
        card(
            "plan-2-3fa9",
            CardKind::PlanReview,
            vec![question(vec![option("execute"), option("revise")], true)],
        ),
        SessionState::default(),
    );
    let cards = &mut feed
        .live
        .get_mut(SESSION)
        .and_then(|live| live.state.as_mut())
        .expect("the session's state")
        .cards;
    for n in 0..3 {
        let mut asks = cards[cards.len() - 1].clone();
        asks.id = format!("c-{n}");
        asks.kind = CardKind::ChildQuestion;
        cards.insert(n, asks);
    }
    let session = Id::from(SESSION);
    let mut state = State::default();
    let dock = super::page(&feed, &state, &session, daemon_env(&feed, Form::Desktop)).dock;
    let line = dock
        .waiting
        .iter()
        .find(|line| line.id == "dock-plan-2-3fa9")
        .expect("the plan review has a line");

    state.act(line.action.clone(), &feed);
    let page = super::page(&feed, &state, &session, daemon_env(&feed, Form::Desktop));
    assert!(page.dock.plan.is_some());
}

/// A plan review left open while another docked choice was brought forward drew both option
/// lists on the dock's keys, and a digit meant for the other card could approve the plan. Only
/// the card brought forward last is open.
#[test]
fn a_plan_review_and_another_docked_choice_are_never_open_together() {
    let mut feed = feed_with(
        card(
            "plan-2-3fa9",
            CardKind::PlanReview,
            vec![question(vec![option("execute"), option("revise")], true)],
        ),
        SessionState::default(),
    );
    let cards = &mut feed
        .live
        .get_mut(SESSION)
        .and_then(|live| live.state.as_mut())
        .expect("the session's state")
        .cards;
    let mut asks = cards[0].clone();
    asks.id = "c-1".into();
    asks.kind = CardKind::ChildQuestion;
    cards.push(asks);
    let session = Id::from(SESSION);
    let mut state = State::default();
    let mut bring = |card: &str| {
        state.act(
            Action::Dock(DockAction::Bring {
                session: Id::from(SESSION),
                card: card.into(),
            }),
            &feed,
        );
        super::page(&feed, &state, &session, daemon_env(&feed, Form::Desktop)).dock
    };

    bring("plan-2-3fa9");
    let asked = bring("c-1");
    assert!(asked.choice.is_some() && asked.plan.is_none());
    let reviewing = bring("plan-2-3fa9");
    assert!(reviewing.plan.is_some() && reviewing.choice.is_none());
}
