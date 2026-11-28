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
        note_on_picks: false,
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
            transcript_seams: Default::default(),
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

#[test]
fn question_draft_sends_pick_notes_and_other_as_distinct_answers() {
    let offered = || vec![option("rest"), option("grpc")];
    let mut feed = feed_with(
        card(
            "q-1",
            CardKind::Question,
            vec![
                CardQuestion {
                    note_on_picks: true,
                    ..question(offered(), true)
                },
                CardQuestion {
                    multi: true,
                    note_on_picks: true,
                    ..question(offered(), false)
                },
                CardQuestion {
                    multi: true,
                    ..question(offered(), true)
                },
            ],
        ),
        SessionState::default(),
    );
    feed.link = crate::model::daemon::Link::Up;
    let session = Id::from(SESSION);
    let mut state = State::default();
    let bring = Action::Dock(DockAction::Bring {
        session: session.clone(),
        card: "q-1".into(),
    });
    let show = |tab| {
        Action::Dock(DockAction::Show {
            session: session.clone(),
            card: "q-1".into(),
            tab,
        })
    };
    let shown = |state: &State| dock_of(&feed, state).answer.unwrap().question.unwrap();
    state.act(bring.clone(), &feed);

    let first = shown(&state);
    assert!(first.note.is_none());
    state.act(first.options[0].action.clone().unwrap(), &feed);
    let first = shown(&state);
    assert_eq!(first.index, 0);
    state.act(first.note.unwrap().with("keep the old route".into()), &feed);
    let first = shown(&state);
    assert!(first.options[0].chosen);
    assert!(first.words.unwrap().text.is_empty());

    state.act(show(1), &feed);
    assert!(shown(&state).words.is_none());
    for index in [0, 1] {
        state.act(shown(&state).options[index].action.clone().unwrap(), &feed);
    }
    state.act(
        shown(&state).note.unwrap().with("in that order".into()),
        &feed,
    );

    state.act(show(2), &feed);
    state.act(shown(&state).options[0].action.clone().unwrap(), &feed);
    let last = shown(&state);
    assert!(last.note.is_none());
    state.act(last.words.unwrap().with("use neither".into()), &feed);
    assert!(shown(&state).options.iter().all(|option| !option.chosen));

    let close = dock_of(&feed, &state).answer.unwrap().close;
    assert!(answers(&state.act(close, &feed)).is_empty());
    state.act(bring, &feed);
    let submit = dock_of(&feed, &state).answer.unwrap().send.unwrap().action;
    // A refreshed card can remove an option or revoke the note capability. Neither case may
    // turn the invalid draft into a new answer, even if Send was armed against the old card.
    for refreshed in [
        CardQuestion {
            note_on_picks: true,
            ..question(vec![option("grpc")], true)
        },
        question(offered(), true),
    ] {
        let mut refreshed_feed = feed_with(
            card("q-1", CardKind::Question, vec![refreshed]),
            SessionState::default(),
        );
        refreshed_feed.link = crate::model::daemon::Link::Up;
        assert!(answers(&state.act(submit.clone(), &refreshed_feed)).is_empty());
        assert!(
            dock_of(&refreshed_feed, &state)
                .answer
                .unwrap()
                .question
                .unwrap()
                .options
                .iter()
                .all(|option| !option.chosen)
        );
    }
    let sent = answers(&state.act(submit, &feed));
    assert_eq!(sent.len(), 1);
    assert!(!sent[0].dismiss);
    assert_eq!(
        serde_json::to_value(&sent[0].answers).unwrap(),
        serde_json::json!([
            { "options": ["rest"], "text": "keep the old route" },
            { "options": ["rest", "grpc"], "text": "in that order" },
            { "text": "use neither" },
        ])
    );
}

/// The dock of `SESSION` as a build's page on desktop builds it.
pub(super) fn dock_of(feed: &Feed, state: &State) -> super::Dock {
    let id = Id::from(SESSION);
    let facts = super::Facts {
        role: super::SessionRole::Build,
        task: None,
    };
    let env = crate::model::health::daemon_env(feed, crate::model::nav::Form::Desktop);
    let cx = super::Cx::new(feed, state.ui(SESSION), &id, env, &facts);
    super::dock(&cx)
}
