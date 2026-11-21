use corral_protocol::app::SESSION_STATE_V1;
use corral_protocol::detail::{CardKind, DetailRef};
use corral_protocol::session_state::{CardBrief, SessionState};

use crate::model::daemon::{ClientError, Feed, detail_key};
use crate::model::effect::Effect;
use crate::model::session::questions_tests::{SESSION, card, dock_of, feed_with, option, question};
use crate::model::session::{Action, Asked, DockAction, State};
use crate::model::{Id, Live};

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

/// Home's Answer opened the session with its question folded to a line, and a failed read of
/// the cards showed only as a dock line under it. An Answer opens the card its row named, open
/// and reading until the host's state briefs it and its body is read, with Retry in that card
/// after a failure; a card the host no longer briefs is not replaced by another.
#[test]
fn an_answer_from_outside_opens_the_named_card_and_never_another() {
    let session = Id::from(SESSION);
    let answer = |card: &str| {
        Action::Dock(DockAction::Answer {
            session: session.clone(),
            asked: Asked::Card(card.into()),
        })
    };
    let mut cold = Feed::default();
    cold.capabilities.insert(SESSION_STATE_V1.into());
    let mut state = State::default();
    assert_eq!(
        state.act(answer("q-2"), &cold),
        vec![Effect::OpenLive(Live::Session(session.clone()))]
    );
    let waiting = dock_of(&cold, &state)
        .answer
        .expect("the named card shows before its brief");
    assert_eq!(waiting.card, "q-2");
    assert!(waiting.question.is_none() && waiting.read.is_some());

    // The host briefs an older question first; the named one opens with its body.
    let mut feed = feed_with(
        card(
            "q-2",
            CardKind::Question,
            vec![question(vec![option("utc"), option("local")], false)],
        ),
        SessionState::default(),
    );
    feed.capabilities.insert(SESSION_STATE_V1.into());
    if let Some(host) = feed
        .live
        .get_mut(SESSION)
        .and_then(|live| live.state.as_mut())
    {
        host.cards.insert(
            0,
            CardBrief {
                id: "q-1".into(),
                kind: CardKind::Question,
                title: "An older question".into(),
                child: None,
                expires_at: None,
            },
        );
    }
    state.settle(&feed);
    let opened = dock_of(&feed, &state).answer.expect("the named card opens");
    assert_eq!(opened.card, "q-2");
    assert!(opened.question.is_some() && opened.read.is_none());

    // Its cards' read failed: the open card says so once, with Retry.
    let target = DetailRef::SessionCards { id: SESSION.into() };
    if let Some(cache) = feed.details.get_mut(&detail_key(&target)) {
        cache.pages.clear();
        cache.merged = None;
        cache.error = Some(ClientError {
            code: "not_found".into(),
            message: "no cards".into(),
        });
    }
    let failed = dock_of(&feed, &state);
    let read = failed
        .answer
        .and_then(|card| card.read)
        .expect("the open card says where its read stands");
    assert!(read.retry.is_some());
    assert!(failed.read.is_none());

    // A card no brief and no row names opens nothing in its place.
    let mut other = State::default();
    other.act(answer("q-9"), &feed);
    other.settle(&feed);
    assert!(dock_of(&feed, &other).answer.is_none());
}
