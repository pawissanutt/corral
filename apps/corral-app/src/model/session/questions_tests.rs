use corral_protocol::detail::{
    CardKind, CardOption, CardQuestion, DetailBody, DetailRef, DetailReply, SessionCard,
    SessionCards,
};
use corral_protocol::session_host::SessionCardAnswer;
use corral_protocol::session_state::{CardBrief, SessionState};

use super::transcript::LiveSession;
use super::{Action, DockAction, State};
use crate::model::Id;
use crate::model::daemon::{DetailCache, Feed, detail_key};
use crate::model::effect::Effect;

pub(super) const SESSION: &str = "B-k3x9";

pub(super) fn option(id: &str) -> CardOption {
    CardOption {
        id: id.into(),
        label: id.into(),
        detail: None,
        unavailable: None,
        needs_text: None,
        destructive: false,
    }
}

pub(super) fn question(options: Vec<CardOption>, free_text: bool) -> CardQuestion {
    CardQuestion {
        header: None,
        text: "Which one?".into(),
        options,
        initial: None,
        multi: false,
        free_text,
    }
}

pub(super) fn card(id: &str, kind: CardKind, questions: Vec<CardQuestion>) -> SessionCard {
    SessionCard {
        id: id.into(),
        kind,
        title: "Waiting on you".into(),
        meta: None,
        body: None,
        body_more: false,
        questions,
        child: None,
        tool: None,
        opened_at: "2026-11-19T12:00:00Z".into(),
        expires_at: None,
    }
}

/// A feed whose session `SESSION` waits on `card`: briefed in the host's state and read.
pub(super) fn feed_with(card: SessionCard, state: SessionState) -> Feed {
    let mut feed = Feed::default();
    let mut live = LiveSession::default();
    live.replace_state(Some(Box::new(SessionState {
        cards: vec![CardBrief {
            id: card.id.clone(),
            kind: card.kind,
            title: card.title.clone(),
            child: None,
            expires_at: None,
        }],
        ..state
    })));
    feed.live.insert(Id::from(SESSION), live);
    let target = DetailRef::SessionCards { id: SESSION.into() };
    let body = DetailBody::SessionCards(SessionCards { cards: vec![card] });
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            target,
            pages: vec![DetailReply {
                etag: "e1".into(),
                as_of: "2026-11-19T12:00:00Z".into(),
                seq: 1,
                next: None,
                body: Some(body.clone()),
            }],
            loading: false,
            stale: false,
            error: None,
            merged: Some(body),
        },
    );
    feed
}

/// The card answers among `effects`.
pub(super) fn answers(effects: &[Effect]) -> Vec<SessionCardAnswer> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::SessionCardAnswer { answer, .. } => Some(answer.clone()),
            _ => None,
        })
        .collect()
}

/// A deferred question card asks several questions; sending it with one answered would leave
/// the host a partial answer it cannot take back, so nothing goes out until each has one.
#[test]
fn a_two_question_card_is_not_sent_until_both_are_answered() {
    let feed = feed_with(
        card(
            "q-1",
            CardKind::Question,
            vec![
                question(vec![option("rest"), option("grpc")], true),
                question(vec![option("memory"), option("disk")], true),
            ],
        ),
        SessionState::default(),
    );
    let session = Id::from(SESSION);
    let mut state = State::default();
    let pick = |question: usize, option: &str| {
        Action::Dock(DockAction::Pick {
            session: session.clone(),
            card: "q-1".into(),
            question,
            option: option.into(),
        })
    };
    let submit = Action::Dock(DockAction::Submit {
        session: session.clone(),
        card: "q-1".into(),
    });

    state.act(pick(0, "rest"), &feed);
    assert!(answers(&state.act(submit.clone(), &feed)).is_empty());

    state.act(pick(1, "disk"), &feed);
    let sent = answers(&state.act(submit, &feed));
    assert_eq!(sent.len(), 1);
    let chosen: Vec<Vec<String>> = sent[0]
        .answers
        .iter()
        .map(|answer| answer.options.clone())
        .collect();
    assert_eq!(chosen, [vec!["rest".to_owned()], vec!["disk".to_owned()]]);
}
