use super::*;
use crate::model::agents::{self, Control};
use crate::model::hue::Hue;

/// corrald refuses Pause for a session without a pi host; the session shows the refusal with
/// corrald's reason, and it is not taken for a connection problem.
#[test]
fn pause_on_a_session_without_a_pi_host_shows_corralds_reason() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let sent = app.operation(
        feed,
        "pause-1".into(),
        control::SESSION_PAUSE,
        Control::Pause.params("T-1_R1"),
    );
    assert_eq!(sent.len(), 1);
    let reason = "session has no Pi socket; use Terminate to stop its host";
    let refusal = Refusal {
        reason: reason.into(),
        next: Vec::new(),
        problems: None,
    };
    app.event(
        feed,
        Event::Reply {
            method: control::SESSION_PAUSE,
            tag: Some("pause-1".into()),
            params: sent[0].1.clone(),
            outcome: Err((corral_protocol::rpc::ErrorCode::Refused, reason.into())),
            error_data: Some(serde_json::to_value(&refusal).unwrap()),
        },
    )
    .unwrap();
    let mut state = agents::State::default();
    state.control_sent(
        Id::from("T-1_R1"),
        Id::from("pause-1"),
        Control::Pause,
        None,
    );
    let shown = feed.with_untracked(|feed| agents::control_status(&state, "T-1_R1", feed));
    assert_eq!(shown, Some((Hue::Fail, format!("Pause refused: {reason}"))));
    assert_eq!(feed.with_untracked(|feed| feed.problem.clone()), None);
}

#[test]
fn ask_holds_its_draft_until_end_is_acknowledged_even_with_a_delayed_row() {
    use crate::model::{Effect, assistant, session};
    use wire::rows::{AssistantRow, AssistantState};
    use wire::session_state::SendPolicy;

    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let sample = crate::model::preview::feed(0, String::new(), String::new(), None);
    let old = Id::from(assistant::quick_id(&sample).unwrap());
    let ready = sample.assistants.get(&old).unwrap().clone();
    let feed = signal(sample);
    let mut client = AppState::new();
    client.ready = true;
    let mut chats = assistant::State::default();
    let mut ask = session::State {
        ask: Some(old.clone()),
        ..Default::default()
    };
    let draft = "unsent follow-up";
    let send = |ask: &mut session::State, chats: &assistant::State, id: &Id| {
        feed.with_untracked(|feed| {
            ask.act_input(
                session::Action::Send {
                    session: id.clone(),
                    text: draft.into(),
                    policy: SendPolicy::Steer,
                    attachments: Vec::new(),
                },
                true,
                chats,
                feed,
            )
        })
    };
    let end = |client: &AppState, chats: &mut assistant::State| {
        let effects = feed.with_untracked(|feed| {
            chats.act_live(assistant::Action::ConfirmEnd(old.to_string()), feed)
        });
        let [Effect::Session { key, op }] = effects.as_slice() else {
            panic!("one End request")
        };
        let outgoing = client.operation(feed, key.to_string(), ASSISTANT_SESSION, value(op));
        (key.clone(), outgoing[0].1.clone())
    };
    let (first, params) = end(&client, &mut chats);
    feed.with_untracked(|feed| {
        ask.act(
            session::Action::Draft {
                session: old.clone(),
                text: draft.into(),
            },
            feed,
        )
    });
    assert!(send(&mut ask, &chats, &old).is_empty());
    assert!(
        feed.with_untracked(|feed| ask.act_slash(
            &old,
            vec![
                session::Action::Draft {
                    session: old.clone(),
                    text: String::new()
                },
                session::Action::Command(session::CommandAction::CloseMenu(old.clone())),
            ],
            true,
            &chats,
            feed,
        ))
        .is_empty()
    );
    assert_eq!(ask.ui(old.as_str()).draft, draft);

    let reason = "host refused End";
    client
        .event(
            feed,
            Event::Reply {
                method: ASSISTANT_SESSION,
                tag: Some(first.to_string()),
                params,
                outcome: Err((wire::rpc::ErrorCode::Refused, reason.into())),
                error_data: None,
            },
        )
        .unwrap();
    let failed = feed.with_untracked(|feed| {
        feed.operations[&first]
            .error
            .as_ref()
            .unwrap()
            .message
            .clone()
    });
    chats.settle(&first, assistant::OpOutcome::Failed(failed));
    feed.with_untracked(|feed| {
        assert_eq!(
            assistant::end_state(&chats, feed, old.as_str()),
            Some(assistant::EndState::Failed(reason.into()))
        );
        assert_eq!(ask.input_block(&old, true, &chats, feed), None);
        assert_eq!(ask.ask_id(), old);
    });
    assert_eq!(ask.ui(old.as_str()).draft, draft);
    assert!(
        matches!(send(&mut ask.clone(), &chats, &old).as_slice(), [Effect::SessionSend { send, .. }] if send.id == old.as_str())
    );

    let quick_detail = DetailRef::Session {
        id: QUICK_THREAD.into(),
    };
    let detail = client.detail_request(
        feed,
        "old-quick-page".into(),
        DetailGet {
            target: quick_detail.clone(),
            etag: None,
            page: None,
        },
    );
    let (second, params) = end(&client, &mut chats);
    assert!(send(&mut ask, &chats, &old).is_empty());
    client
        .event(
            feed,
            Event::Reply {
                method: ASSISTANT_SESSION,
                tag: Some(second.to_string()),
                params,
                outcome: Ok(value(AssistantRow {
                    state: AssistantState::Ended,
                    ..ready.clone()
                })),
                error_data: None,
            },
        )
        .unwrap();
    // The old composer can still be mounted as its state catches up with the receipt.
    assert!(send(&mut ask, &chats, &old).is_empty());
    feed.with_untracked(|feed| ask.settle(feed));
    let quick = Id::from(QUICK_THREAD);
    assert_eq!(ask.ask_id(), quick);
    assert_eq!(ask.ui(QUICK_THREAD).draft, draft);

    let mut old_page = empty_quick_page();
    old_page.id = old.to_string();
    let reread = client
        .event(
            feed,
            Event::Reply {
                method: DETAIL_GET,
                tag: Some("old-quick-page".into()),
                params: detail[0].1.clone(),
                outcome: Ok(value(DetailReply {
                    etag: "before-end".into(),
                    as_of: String::new(),
                    seq: 1,
                    next: None,
                    body: Some(DetailBody::Session(old_page)),
                })),
                error_data: None,
            },
        )
        .unwrap();
    assert!(matches!(reread.as_slice(), [(DETAIL_GET, _, _)]));
    assert!(feed.with_untracked(|feed| {
        feed.detail(&quick_detail)
            .and_then(DetailCache::body)
            .is_none()
    }));

    // A subscription page captured before End must not put the old thread back under quick.
    let delta = SubDelta::Assistant {
        delta: RowsDelta {
            epoch: 1,
            rows: vec![ready],
            left: Vec::new(),
            cursor: BoardCursor::default(),
            more: false,
        },
    };
    client.rows(feed, delta, true).unwrap();
    assert!(
        feed.with_untracked(|feed| assistant::quick_id(feed))
            .is_none()
    );
    assert!(send(&mut ask, &chats, &old).is_empty());
    assert!(
        matches!(send(&mut ask, &chats, &quick).as_slice(), [Effect::SessionSend { send, .. }] if send.id == QUICK_THREAD && send.text == draft)
    );
}
