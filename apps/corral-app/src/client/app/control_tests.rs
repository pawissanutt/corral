use super::*;
use crate::model::agents::{self, Control};
use crate::model::hue::Hue;

/// The native and paired drivers consult `handles` before delivering an operation reply.
/// A missing route leaves an enrolled device stuck on Reading even though corrald answered.
#[test]
fn pi_hosted_tools_routes_preference_replies_to_the_waiting_form() {
    for method in [PI_HOSTED_TOOLS, PI_HOSTED_TOOL_MODELS, PI_STARTUP] {
        assert!(handles(&Event::Reply {
            method,
            tag: Some("settings-read".into()),
            params: json!({"change": null}),
            outcome: Ok(json!({"revision": "r1"})),
            error_data: None,
        }));
    }
    let mut feed = crate::model::preview::feed(0, String::new(), String::new(), None);
    let unavailable = crate::model::settings::pi_hosted_tools::unavailable;
    assert!(unavailable(&feed).is_none());
    feed.capabilities.remove(wire::config::PI_HOSTED_TOOLS_V1);
    assert!(unavailable(&feed).is_some());
    assert!(unavailable(&Feed::default()).is_some());
}

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
fn a_new_quick_thread_loads_its_metadata_without_replacing_the_stream() {
    use wire::detail::{HostPresence, SessionItem, SessionMessage, SessionPage};
    use wire::rows::{AssistantLocation, AssistantState};
    use wire::subscription::{TranscriptOp, TranscriptPart};

    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let sample = crate::model::preview::feed(0, String::new(), String::new(), None);
    let old = crate::model::assistant::row(&sample, QUICK_THREAD).unwrap();
    let quick = "A-999".to_owned();
    let row = wire::rows::AssistantRow {
        id: quick.clone(),
        state: AssistantState::Ready,
        live_in: AssistantLocation::Daemon,
        ..old.clone()
    };
    let feed = signal(Feed::default());
    let mut client = AppState::new();
    client.ready = true;
    client.session_scopes.insert(QUICK_THREAD.into(), 1);
    let target = DetailRef::Session {
        id: QUICK_THREAD.into(),
    };
    let key = detail_key(&target);
    let mut prior = empty_quick_page();
    prior.id.clone_from(&old.id);
    let mut cache = DetailCache {
        target: target.clone(),
        pages: vec![],
        merged: Some(DetailBody::Session(prior)),
        loading: false,
        stale: false,
        error: None,
        transcript_seams: Default::default(),
    };
    let mut message: SessionMessage = serde_json::from_value(json!({
        "id": format!("{quick}/open/1.1.1"),
        "author": "assistant",
        "items": [value(SessionItem::Text { text: "streamed reply".into() })]
    }))
    .unwrap();
    crate::model::session::transcript::mirror::apply(
        &mut cache,
        vec![
            TranscriptOp::Reset { older: None },
            TranscriptOp::Put {
                message: message.clone(),
                before: None,
                replaces: None,
                open: true,
                cut: false,
                again: false,
            },
        ],
        false,
    );
    feed.update(|f| {
        f.capabilities.insert(SESSION_TRANSCRIPT_V1.into());
        f.details.insert(key.clone(), cache);
    });
    let delta = |row| SubDelta::Assistant {
        delta: RowsDelta {
            epoch: 1,
            rows: vec![
                wire::rows::AssistantRow {
                    state: AssistantState::Ended,
                    ..old.clone()
                },
                row,
            ],
            left: vec![],
            cursor: BoardCursor::default(),
            more: false,
        },
    };
    let respond = |client: &mut AppState, reads: Vec<Outgoing>, page: SessionPage| {
        let [(DETAIL_GET, params, tag)] = reads.as_slice() else {
            panic!("expected a quick metadata read, got {reads:?}");
        };
        client
            .event(
                feed,
                Event::Reply {
                    method: DETAIL_GET,
                    tag: tag.clone(),
                    params: params.clone(),
                    outcome: Ok(value(DetailReply {
                        etag: "thread".into(),
                        as_of: String::new(),
                        seq: 1,
                        next: None,
                        body: Some(DetailBody::Session(page)),
                    })),
                    error_data: None,
                },
            )
            .unwrap()
    };
    let reads = client
        .rows(
            feed,
            delta(wire::rows::AssistantRow {
                state: AssistantState::Cold,
                live_in: AssistantLocation::None,
                ..row.clone()
            }),
            true,
        )
        .unwrap();
    let retry = respond(&mut client, reads, empty_quick_page());
    let mut page = empty_quick_page();
    page.id.clone_from(&quick);
    assert!(respond(&mut client, retry, page.clone()).is_empty());

    // The End row was late: the new open id must still accept appends.
    feed.update(|f| {
        crate::model::session::transcript::mirror::apply(
            f.details.get_mut(&key).unwrap(),
            vec![TranscriptOp::Append {
                id: message.id.clone(),
                part: TranscriptPart::Text,
                offset: "streamed reply".len() as u64,
                text: " grows".into(),
            }],
            false,
        );
    });
    message.items = vec![SessionItem::Text {
        text: "streamed reply grows".into(),
    }];

    let reads = client.rows(feed, delta(row.clone()), true).unwrap();
    let retry = respond(&mut client, reads, page.clone());
    page.host = HostPresence::Running;
    assert!(respond(&mut client, retry, page).is_empty());
    feed.with_untracked(|f| {
        let Some(DetailBody::Session(page)) = f.detail(&target).and_then(DetailCache::body) else {
            panic!("expected quick transcript");
        };
        assert_eq!((&page.id, page.host), (&quick, HostPresence::Running));
        assert_eq!(page.messages, [message]);
    });
    assert!(
        client
            .rows(feed, delta(row.clone()), true)
            .unwrap()
            .is_empty()
    );

    // The second chat can also end before metadata catches up with the stream.
    feed.update(|f| {
        if let Some(DetailBody::Session(page)) = &mut f.details.get_mut(&key).unwrap().merged {
            page.id.clone_from(&old.id);
        }
    });
    client
        .rows(
            feed,
            delta(wire::rows::AssistantRow {
                state: AssistantState::Ended,
                ..row
            }),
            true,
        )
        .unwrap();
    feed.with_untracked(|f| {
        assert_eq!(
            f.detail(&target).and_then(DetailCache::body),
            Some(&DetailBody::Session(empty_quick_page()))
        );
    });
}

#[test]
fn a_quick_host_start_refreshes_metadata_after_an_inflight_older_page() {
    use wire::detail::{HostPresence, SessionMessage, SessionPage};
    use wire::rows::{AssistantLocation, AssistantState};
    use wire::subscription::TranscriptOp;

    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let sample = crate::model::preview::feed(0, String::new(), String::new(), None);
    let row = crate::model::assistant::row(&sample, QUICK_THREAD).unwrap();
    let target = DetailRef::Session {
        id: QUICK_THREAD.into(),
    };
    let key = detail_key(&target);
    let message = |suffix: &str| -> SessionMessage {
        serde_json::from_value(json!({
            "id": format!("{}/{suffix}", row.id), "author": "assistant",
            "items": [{"kind": "text", "text": suffix}]
        }))
        .unwrap()
    };
    let (older, head, open) = (
        message("entry/older"),
        message("entry/head"),
        message("open/1.1.1"),
    );
    let mut cold = empty_quick_page();
    cold.id.clone_from(&row.id);
    cold.messages = vec![head.clone()];
    cold.older = Some("before-head".into());
    let mut cache = DetailCache {
        target: target.clone(),
        pages: vec![],
        merged: Some(DetailBody::Session(cold.clone())),
        loading: false,
        stale: false,
        error: None,
        transcript_seams: Default::default(),
    };
    crate::model::session::transcript::mirror::apply(
        &mut cache,
        vec![TranscriptOp::Put {
            message: open.clone(),
            before: None,
            replaces: None,
            open: true,
            cut: false,
            again: false,
        }],
        false,
    );
    let feed = signal(Feed::default());
    feed.update(|f| {
        f.capabilities.insert(SESSION_TRANSCRIPT_V1.into());
        f.details.insert(key, cache);
    });
    let mut client = AppState::new();
    client.ready = true;
    client.session_scopes.insert(QUICK_THREAD.into(), 1);
    let older_read = client.detail_request(
        feed,
        "older".into(),
        DetailGet {
            target: target.clone(),
            etag: None,
            page: cold.older.clone(),
        },
    );
    assert!(
        client
            .rows(
                feed,
                SubDelta::Assistant {
                    delta: RowsDelta {
                        epoch: 1,
                        rows: vec![wire::rows::AssistantRow {
                            state: AssistantState::Ready,
                            live_in: AssistantLocation::Daemon,
                            ..row.clone()
                        }],
                        left: vec![],
                        cursor: BoardCursor::default(),
                        more: false,
                    }
                },
                true
            )
            .unwrap()
            .is_empty(),
        "the metadata read must not interrupt Older"
    );
    let respond = |client: &mut AppState, reads: Vec<Outgoing>, page: SessionPage| {
        let [(DETAIL_GET, params, tag)] = reads.as_slice() else {
            panic!("expected a single detail read, got {reads:?}");
        };
        client
            .event(
                feed,
                Event::Reply {
                    method: DETAIL_GET,
                    tag: tag.clone(),
                    params: params.clone(),
                    outcome: Ok(value(DetailReply {
                        etag: "quick".into(),
                        as_of: String::new(),
                        seq: 1,
                        next: None,
                        body: Some(DetailBody::Session(page)),
                    })),
                    error_data: None,
                },
            )
            .unwrap()
    };
    let metadata = respond(
        &mut client,
        older_read,
        SessionPage {
            messages: vec![older.clone()],
            older: Some("before-older".into()),
            ..cold.clone()
        },
    );
    let [(DETAIL_GET, params, _)] = metadata.as_slice() else {
        panic!("the running row must trigger a metadata read when Older settles");
    };
    let request: DetailGet = decode(params.clone()).unwrap();
    assert_eq!(request.target, target);
    assert!(request.page.is_none());
    assert!(
        respond(
            &mut client,
            metadata,
            SessionPage {
                host: HostPresence::Running,
                ..cold
            }
        )
        .is_empty()
    );
    feed.with_untracked(|f| {
        let Some(DetailBody::Session(page)) = f.detail(&target).and_then(DetailCache::body) else {
            panic!("expected Quick transcript");
        };
        assert_eq!(page.host, HostPresence::Running);
        assert_eq!(page.older.as_deref(), Some("before-older"));
        assert_eq!(page.messages, [older, head, open]);
    });
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
                    restore: None,
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
