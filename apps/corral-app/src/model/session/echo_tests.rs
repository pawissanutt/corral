use corral_protocol::detail::{
    DetailBody, DetailRef, HostPresence, InputFacts, SessionItem, SessionMessage, SessionPage,
};
use corral_protocol::rows::{AssistantRow, AssistantState};
use corral_protocol::session_host::{InputDisposition, SessionSent};
use corral_protocol::session_state::{InputOrigin, SendPolicy};

use super::{Action, State};
use crate::model::Id;
use crate::model::assistant::QUICK;
use crate::model::daemon::{self, DetailCache, Feed, Operation};
use crate::model::effect::Effect;

const SCOPE: &str = "A-7";

fn owner(id: &str, text: &str, input: Option<&str>) -> SessionMessage {
    SessionMessage {
        id: id.into(),
        author: "user".into(),
        at: Some("2026-11-19T10:00:00Z".into()),
        items: vec![SessionItem::Text { text: text.into() }],
        model: None,
        stop_reason: None,
        usage: None,
        input: input.map(|input| InputFacts {
            id: Some(input.into()),
            origin: InputOrigin::Owner,
            typed_at: None,
            device: None,
            to: None,
        }),
        attachments: Vec::new(),
    }
}

fn serve(feed: &mut Feed, messages: Vec<SessionMessage>) {
    let target = DetailRef::Session { id: SCOPE.into() };
    feed.details.insert(
        daemon::detail_key(&target),
        DetailCache {
            transcript_seams: Default::default(),
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::Session(SessionPage {
                id: SCOPE.into(),
                role: "assistant".into(),
                task: None,
                run: None,
                machine: None,
                messages,
                older: None,
                replyable: true,
                reply_refusal: None,
                host: HostPresence::Running,
            })),
        },
    );
}

fn echoes(state: &State, feed: &Feed) -> Vec<String> {
    let page = super::transcript::page(feed.detail(&DetailRef::Session { id: SCOPE.into() }));
    super::echo::shown(&Id::from(SCOPE), state.ui(SCOPE), page, feed)
        .into_iter()
        .map(|echo| echo.input)
        .collect()
}

/// Sends `text` and returns its command id, the input id the host keeps.
fn send(state: &mut State, feed: &Feed, scope: &str, text: &str) -> String {
    let effects = state.act(
        Action::Send {
            session: Id::from(scope),
            text: text.into(),
            policy: SendPolicy::Steer,
            attachments: Vec::new(),
        },
        feed,
    );
    match effects.as_slice() {
        [Effect::SessionSend { key, send }] => {
            assert_eq!(send.id, scope);
            assert_eq!(send.cmd_id, key.as_str());
            key.to_string()
        }
        other => panic!("a send goes out once under its command id: {other:?}"),
    }
}

fn ack_started(feed: &mut Feed, key: &str, session: &str) {
    feed.operations.insert(
        Id::from(key),
        Operation {
            method: "session.send".into(),
            loading: false,
            value: serde_json::to_value(SessionSent {
                session: session.into(),
                entry: format!("{session}/input/{key}"),
                disposition: InputDisposition::Started,
                turn: Some(1),
            })
            .ok(),
            error: None,
            refusal: None,
        },
    );
}

fn quick_row(state: AssistantState) -> AssistantRow {
    serde_json::from_value(serde_json::json!({
        "id": SCOPE, "kind": "quick", "title": "Quick thread", "state": state,
        "live_in": "none", "profile": null, "connection": null, "focus": null, "pending": 0,
        "turn": 1, "cost_usd": null, "cost_basis": null, "wait": null, "rev": 2,
    }))
    .expect("a quick chat row")
}

/// The same words served under another id (a message typed in the terminal, an older entry)
/// must not retire the echo: only the served message carrying the send's own input id does.
#[test]
fn an_echo_is_retired_by_its_input_id_never_by_its_text() {
    let mut feed = Feed::default();
    let mut state = State::default();
    serve(&mut feed, Vec::new());
    let key = send(&mut state, &feed, SCOPE, "list the files");
    assert_eq!(echoes(&state, &feed), [key.clone()]);

    serve(
        &mut feed,
        vec![owner("A-7/entry/e1", "list the files", None)],
    );
    state.settle(&feed);
    assert_eq!(
        echoes(&state, &feed),
        [key.clone()],
        "a message with the same text but another id is not this send"
    );

    serve(
        &mut feed,
        vec![
            owner("A-7/entry/e1", "list the files", None),
            owner(
                &format!("A-7/input/{key}"),
                "list the files, edited by the host",
                Some(&key),
            ),
        ],
    );
    state.settle(&feed);
    assert!(echoes(&state, &feed).is_empty());
    assert!(
        state.ui(SCOPE).echoes.is_empty(),
        "a served send is retired, not hidden"
    );
}

/// A send the host consumed without a message (a child steer, an answer, a slash command) is
/// retired by its acknowledgement; it never waits for a transcript row that will not come.
#[test]
fn a_handled_send_is_retired_by_its_acknowledgement() {
    let mut feed = Feed::default();
    let mut state = State::default();
    serve(&mut feed, Vec::new());
    let key = send(&mut state, &feed, SCOPE, "/compact");
    let ack = SessionSent {
        session: SCOPE.into(),
        entry: format!("A-7/input/{key}"),
        disposition: InputDisposition::Handled,
        turn: None,
    };
    feed.operations.insert(
        Id::from(key.as_str()),
        Operation {
            method: "session.send".into(),
            loading: false,
            value: serde_json::to_value(ack).ok(),
            error: None,
            refusal: None,
        },
    );
    assert!(echoes(&state, &feed).is_empty());
}

/// Ask's first send goes to `quick` with no chat yet; corrald creates one and acknowledges the
/// send under it. Ask must follow that chat (its draft and the echo with it), retire the echo
/// once the chat's transcript serves the input id, and go back to `quick` once the chat ended,
/// so the next send starts a new one rather than reaching the ended chat.
#[test]
fn a_quick_send_with_no_session_keys_ask_to_the_returned_id() {
    let mut feed = Feed::default();
    let mut state = State::default();
    assert_eq!(state.ask_id().as_str(), "quick");
    let key = send(&mut state, &feed, QUICK, "what blocks T-12?");
    state.act(
        Action::Draft {
            session: state.ask_id(),
            text: "and T-14?".into(),
        },
        &feed,
    );
    ack_started(&mut feed, &key, SCOPE);
    state.settle(&feed);
    assert_eq!(state.ask_id().as_str(), SCOPE);
    assert_eq!(state.ui(SCOPE).draft, "and T-14?");
    assert!(state.ui("quick").echoes.is_empty());
    assert_eq!(
        echoes(&state, &feed),
        [key.clone()],
        "the echo stays until the chat's transcript serves its input id"
    );

    serve(
        &mut feed,
        vec![owner(
            &format!("{SCOPE}/input/{key}"),
            "what blocks T-12?",
            Some(&key),
        )],
    );
    state.settle(&feed);
    assert!(echoes(&state, &feed).is_empty());

    feed.assistants
        .insert(Id::from(SCOPE), quick_row(AssistantState::Ended));
    state.settle(&feed);
    assert_eq!(
        state.ask_id().as_str(),
        "quick",
        "an ended chat takes no more messages; Ask's next send starts a new one"
    );
}

#[test]
fn a_quick_send_acknowledged_after_end_does_not_echo_in_the_next_thread() {
    let mut feed = Feed::default();
    feed.assistants
        .insert(Id::from(SCOPE), quick_row(AssistantState::Ready));
    let mut state = State::default();
    let old = send(&mut state, &feed, QUICK, "old thread's message");

    // Ask was opened on the old chat by alias. End beats its send acknowledgement.
    feed.assistants
        .insert(Id::from(SCOPE), quick_row(AssistantState::Ended));
    state.settle(&feed);
    let fresh_text = "first message in the new thread";
    let fresh = send(&mut state, &feed, QUICK, fresh_text);
    let quick = Id::from(QUICK);
    let draft = "a draft still unsent";
    state.act(
        Action::Draft {
            session: quick.clone(),
            text: draft.into(),
        },
        &feed,
    );

    // Retiring the old echo must not touch the fresh send still awaiting its own ack.
    ack_started(&mut feed, &old, SCOPE);
    let empty = daemon::empty_quick_page();
    let shown = super::echo::shown(&quick, state.ui(QUICK), Some(&empty), &feed);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].input, fresh);
    state.settle(&feed);
    assert_eq!(state.ask_id(), quick);
    assert_eq!(state.ui(QUICK).echoes.len(), 1);
    assert_eq!(state.ui(QUICK).draft, draft);

    let next = Id::from("A-8");
    ack_started(&mut feed, &fresh, next.as_str());
    state.settle(&feed);
    assert_eq!(state.ask_id(), next);
    assert_eq!(state.ui(next.as_str()).draft, draft);
    let shown = super::echo::shown(&next, state.ui(next.as_str()), None, &feed);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].input, fresh);
    assert_eq!(shown[0].text, fresh_text);
}
