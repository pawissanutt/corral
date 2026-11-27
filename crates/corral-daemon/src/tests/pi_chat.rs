use super::fake_host::{AT, Host, PID, Placement, Script};
use super::*;
use corral_protocol::{
    assistant::*,
    detail::{DetailBody, DetailReply, HostPresence, SessionItem, SessionPage},
    session::*,
    session_host::{
        InputDisposition, SESSION_CARD_ANSWER, SESSION_SEND, SendPolicy, SessionCardAnswered,
        SessionSend, SessionSent,
    },
    subscription::SubDelta,
};
use corral_types::SessionRef;

fn watch(path: &Path, id: &str) -> Client {
    let mut subscriber = Client::connect(path);
    subscriber.ok::<()>(wire::SUB_OPEN, json!({"topic":"session", "scope":id}));
    subscriber
}

fn live_page(subscriber: &mut Client) -> SubDelta {
    let frame = read_frame(&mut subscriber.0).unwrap().unwrap();
    let notification: rpc::Notification<SubDelta> = serde_json::from_slice(&frame.payload).unwrap();
    let SubDelta::Session { scope, cursor, .. } = &notification.params else {
        panic!("expected session page");
    };
    subscriber.ok::<()>(
        wire::SUB_ACK,
        json!({"topic":"session", "scope":scope, "cursor":cursor}),
    );
    notification.params
}

fn text(subscriber: &mut Client, expected: &str) {
    loop {
        let SubDelta::Session { events, .. } = live_page(subscriber) else {
            panic!("expected session page");
        };
        if events.iter().any(|event| matches!(&event.kind, wire::subscription::SessionLiveKind::LiveText { text, .. } if text == expected)) {
            return;
        }
    }
}

/// A `session.transcript.v1` client is sent the journal's tail and then the words being
/// written as an open message, in place of live_text events.
fn mirrors(path: &Path, id: &str) {
    use wire::subscription::{SessionLiveKind, TranscriptOp};
    let mut mirror = Client::connect_with(path, vec![wire::app::SESSION_TRANSCRIPT_V1.into()]);
    mirror.ok::<()>(wire::SUB_OPEN, json!({"topic":"session", "scope":id}));
    let mut ops = Vec::new();
    while !ops
        .iter()
        .any(|op| matches!(op, TranscriptOp::Append { text, .. } if text == "worker live text"))
    {
        let SubDelta::Session {
            events, transcript, ..
        } = live_page(&mut mirror)
        else {
            panic!("expected session page");
        };
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.kind, SessionLiveKind::LiveText { .. }))
        );
        ops.extend(transcript);
    }
    assert!(matches!(ops[0], TranscriptOp::Reset { .. }));
    let recorded = |op: &TranscriptOp| {
        matches!(op, TranscriptOp::Put { message, open: false, .. } if message.items.iter().any(
            |item| matches!(item, SessionItem::Text { text } if text == "recorded **Pi** text")
        ))
    };
    let open = |op: &TranscriptOp| matches!(op, TranscriptOp::Put { open: true, .. });
    assert!(ops.iter().position(recorded) < ops.iter().position(open));
}

#[test]
fn worker_live_replay_cancel_and_paused_engine_send_back_share_the_session() {
    let fixture = Fixture::with_repository();
    let path = fixture.unix_server();
    let mut owner = Client::connect(&path);
    let task = owner.new_task("exercise Pi controls", Vec::new());
    owner.op(dispatch(&task), false);
    let paused = || {
        let rows = crate::views::RowCache::default()
            .get(fixture.shared(), wire::subscription::Topic::Agents)
            .unwrap();
        let crate::views::Rows::Agents(rows) = rows.as_ref() else {
            panic!("expected agent rows");
        };
        rows[0].paused
    };
    assert_eq!(paused(), None);
    let no_pi: rpc::Error = owner
        .call(SESSION_CANCEL, json!({"id":format!("{task}_R1")}))
        .unwrap_err();
    assert_eq!(no_pi.code, rpc::ErrorCode::Refused);
    assert!(no_pi.message.contains("no Pi socket"));

    let engine = fixture.shared().engine().unwrap();
    let mut run = engine.store.list_runs(&task).unwrap().pop().unwrap();
    let run_dir = fixture
        .root()
        .join(crate::STATE_DIR)
        .join("runs")
        .join(&task)
        .join(&run.id);
    let id = format!("{task}_{}", run.id);
    let host = Host::start(
        fixture.root().join("p.sock"),
        id.clone(),
        "worker",
        &run_dir,
        Script::default(),
    );
    run.session = Some(SessionRef::Pi {
        socket: host.socket.clone(),
        inner: Box::new(run.session.take().unwrap()),
        dir: run_dir,
    });
    engine.store.update_run(&run).unwrap();
    assert_eq!(paused(), Some(false));
    {
        let mut state = host.state.lock().unwrap();
        state.open = true;
        state.emit("worker live text");
    }
    crate::topics::start_session_live(fixture.shared()).unwrap();
    text(&mut watch(&path, &id), "worker live text");
    mirrors(&path, &id);
    let page: DetailReply = owner.ok(wire::app::DETAIL_GET, json!({"kind":"session", "id":id}));
    let Some(DetailBody::Session(page)) = page.body else {
        panic!("expected session detail")
    };
    assert!(
        page.messages.iter().flat_map(|m| &m.items).any(
            |item| matches!(item, SessionItem::Text { text } if text == "recorded **Pi** text")
        )
    );
    let cancelled: SessionCancelReply = owner.ok(SESSION_CANCEL, json!({"id":id}));
    assert!(cancelled.cancelled);
    assert!(!host.state.lock().unwrap().open);
    owner.ok::<Value>(SESSION_PAUSE, json!({"id":id}));
    assert_eq!(paused(), Some(true));
    fs::write(run.worktree.join("note"), "").unwrap();
    engine.done(&task, "red gate").unwrap();
    engine.advance(&task).unwrap();
    {
        let state = host.state.lock().unwrap();
        assert!(state.text.is_empty());
        assert_eq!(state.held.len(), 1);
        assert!(state.held[0].contains("test -s note"));
    }
    owner.ok::<Value>(SESSION_RESUME, json!({"id":id}));
    {
        let state = host.state.lock().unwrap();
        assert!(state.held.is_empty());
        assert_eq!(state.text.len(), 1);
    }
    let listed: RewindReply = owner.ok(SESSION_REWIND, json!({"id":id, "action":"list"}));
    assert!(matches!(listed, RewindReply::List { checkpoints, .. } if checkpoints[0].turn == 1));
    let rewound: RewindReply =
        owner.ok(SESSION_REWIND, json!({"id":id, "action":"turn", "turn":1}));
    assert!(matches!(
        rewound,
        RewindReply::Turn(Rewound { turn: 1, .. })
    ));
}

#[test]
fn build_rewind_routes_list_and_turn_to_the_selected_host() {
    let fixture = Fixture::new();
    let build = |id: &str, socket_name: &str| {
        let dir = fixture.root().join("pi-data/build").join(id);
        let socket = fixture.root().join(socket_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("session.json"),
            json!({
                "version": 1, "id": id, "name": "walk", "workspaceRoot": fixture.root(),
                "repoRoot": fixture.root().join("repo"),
                "worktree": fixture.root().join("repo/.worktrees/build-walk"),
                "project": "demo", "here": false, "branch": "build/walk", "base": "base-commit",
                "created": AT, "updated": AT, "socket": socket, "pid": PID,
                "sessionFile": dir.join("pi.jsonl"),
                "route": {"provider": "anthropic-api", "model": "claude-opus-5-5", "effort": "high"},
            })
            .to_string(),
        )
        .unwrap();
        Host::start(socket, id.into(), "build", &dir, Script::default())
    };
    let selected = build("B-rewind", "rewind-build.sock");
    let other = build("B-other", "other-build.sock");
    let mut owner = Client::connect_with(&fixture.unix_server(), vec![wire::BUILD_V1.into()]);

    let listed: RewindReply = owner.ok(SESSION_REWIND, json!({"id":"B-rewind", "action":"list"}));
    let RewindReply::List {
        checkpoints,
        rewinds,
    } = listed
    else {
        panic!("expected rewind list");
    };
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].id, "turn-1");
    assert_eq!(checkpoints[0].turn, 1);
    assert!(rewinds.is_empty());

    let rewound: RewindReply = owner.ok(
        SESSION_REWIND,
        json!({"id":"B-rewind", "action":"turn", "turn":1}),
    );
    let rewound = serde_json::to_value(rewound).unwrap();
    assert_eq!(rewound["kind"], "turn");
    assert_eq!(rewound["turn"], 1);
    assert_eq!(rewound["checkpoint"], "turn-1");
    assert_eq!(rewound["rescue_ref"], "refs/worktree/corral/rescue/1");
    assert_eq!(rewound["from_head"], "new");
    assert_eq!(rewound["head"], "old");
    assert_eq!(rewound["at"], AT);
    assert_eq!(
        selected.state.lock().unwrap().requests,
        vec![
            ("rewind".into(), json!({"list":true})),
            ("rewind".into(), json!({"turn":1}))
        ]
    );
    assert!(other.state.lock().unwrap().requests.is_empty());
}

fn chat_fixture() -> (Fixture, Placement) {
    let placement = Placement::new(|command, dir| {
        assert_eq!(command.get_program(), "corral-pi");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args[0], "chat");
        assert_eq!(args[1], "--dir");
        assert_eq!(Path::new(args[2]), dir);
        assert_eq!(args[3], "--socket");
        let fallback = [
            "--fallback",
            "anthropic-api/claude-sonnet-4-6",
            "--fallback",
            "openai/gpt-5.4",
        ];
        assert!(args.ends_with(&fallback.map(std::ffi::OsStr::new)));
        assert!(
            args.windows(2)
                .any(|args| { args[0] == "--model" && args[1] == "anthropic-api/claude-opus-5-5" })
        );
        assert!(command.get_envs().any(|(key, value)| {
            key == "CORRAL_ACTOR" && value.is_some_and(|value| value == "assistant")
        }));
        let host = Host::start(
            PathBuf::from(args[4]),
            dir.file_name().unwrap().to_str().unwrap().into(),
            "assistant",
            dir,
            Script::default(),
        );
        fs::write(
            dir.join("session.json"),
            json!({"pid": PID, "sessionFile": host.transcript}).to_string(),
        )
        .unwrap();
        Some(host)
    });
    let runner = placement.clone();
    let fixture = Fixture::with_runner(Arc::new(move || Box::new(runner.clone())));
    fs::write(
        fixture.root().join(crate::STATE_DIR).join("corral.toml"),
        r#"
[profiles.pi]
harness = "pi"
model = "anthropic-api/claude-opus-5-5"
fallback = ["anthropic-api/claude-sonnet-4-6", "openai/gpt-5.4"]
[profiles.secondary]
harness = "pi"
[defaults]
auto_dispatch = false
auto_land = false
"#,
    )
    .unwrap();
    (fixture, placement)
}

fn quick_send(cmd: &str, text: &str) -> SessionSend {
    SessionSend {
        id: QUICK_THREAD.into(),
        cmd_id: cmd.into(),
        text: text.into(),
        policy: SendPolicy::Steer,
        to: None,
        typed_at: Some(AT.into()),
        attachments: Vec::new(),
    }
}

fn session_page(owner: &mut Client, id: &str) -> SessionPage {
    let detail: DetailReply = owner.ok(wire::app::DETAIL_GET, json!({"kind": "session", "id": id}));
    let Some(DetailBody::Session(page)) = detail.body else {
        panic!("expected session detail");
    };
    page
}

#[test]
fn an_empty_quick_scope_serves_an_empty_page_and_the_first_send_creates_the_chat() {
    let (fixture, _placement) = chat_fixture();
    let path = fixture.unix_server();
    crate::topics::start_session_live(fixture.shared()).unwrap();
    let mut owner = Client::connect(&path);
    for method in [
        "assistant.send",
        "assistant.cancel",
        "assistant.stop",
        "prompt.answer",
    ] {
        let error = owner.call(method, json!({})).unwrap_err();
        assert_eq!(error.code, rpc::ErrorCode::MethodNotFound);
    }
    let mut live = watch(&path, QUICK_THREAD);

    let empty = |page: SessionPage, delta| {
        assert_eq!(page.id, QUICK_THREAD);
        assert_eq!(page.role, "assistant");
        assert_eq!(page.host, HostPresence::NotRunning);
        assert!(page.replyable);
        assert!(page.reply_refusal.is_none());
        assert!(page.messages.is_empty());
        assert!(page.older.is_none());
        assert!(matches!(
            delta,
            SubDelta::Session {
                scope, generation: None, events, more: false, gap: None, fleet: None,
                state: None, cursor, ..
            } if scope == QUICK_THREAD && events.is_empty() && cursor.rev == 0
        ));
    };
    empty(session_page(&mut owner, QUICK_THREAD), live_page(&mut live));
    assert!(
        crate::assistant::rows(fixture.shared(), false)
            .unwrap()
            .is_empty()
    );

    let sent: SessionSent = owner.ok(SESSION_SEND, quick_send("send", "app input"));
    assert_eq!(sent.session, "A-1");
    assert_eq!(sent.entry, "A-1/input/send");
    assert_eq!(sent.disposition, InputDisposition::Started);
    text(&mut live, "assistant live text");
    let page = session_page(&mut owner, QUICK_THREAD);
    assert_eq!(page.id, sent.session);
    let texts: Vec<_> = page
        .messages
        .iter()
        .flat_map(|row| &row.items)
        .filter_map(|item| match item {
            SessionItem::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        [
            "terminal input",
            "recorded **Pi** text",
            "app input",
            "assistant live text",
        ],
    );
    assert_eq!(page.messages[0].id, "A-1/entry/terminal");
    assert_eq!(page.messages[2].id, sent.entry);

    owner.ok::<Value>(
        ASSISTANT_SESSION,
        AssistantSessionOp::End {
            session: sent.session.clone(),
        },
    );
    let ended = session_page(&mut owner, &sent.session);
    assert!(!ended.replyable);
    assert_eq!(ended.messages, page.messages);
    let reset = live_page(&mut live);
    // The poller can observe socket shutdown before End persists the record.
    let reset = if matches!(
        &reset,
        SubDelta::Session {
            generation: None,
            gap: Some(_),
            ..
        }
    ) {
        live_page(&mut live)
    } else {
        reset
    };
    empty(session_page(&mut owner, QUICK_THREAD), reset);
}

#[test]
fn quick_chat_lifecycle_answers_and_restarts_without_automation() {
    let (fixture, placement) = chat_fixture();
    crate::topics::start_session_live(fixture.shared()).unwrap();
    let path = fixture.unix_server();
    let mut owner = Client::connect(&path);
    let welcome = crate::app::dispatch::welcome(
        fixture.shared(),
        wire::Hello {
            proto: wire::Proto {
                major: wire::PROTO_MAJOR,
                minor: wire::PROTO_MINOR,
            },
            client: wire::ClientInfo {
                kind: "app".into(),
                version: "test".into(),
            },
            caps: vec![wire::ASSISTANT_V1.into()],
        },
    );
    assert_eq!(welcome.caps, [wire::ASSISTANT_V1]);
    let first: SessionSent = owner.ok(SESSION_SEND, quick_send("first", "first chat"));
    assert_eq!(first.session, "A-1");
    assert_eq!(first.entry, "A-1/input/first");
    text(&mut watch(&path, "A-1"), "assistant live text");
    let state = placement.state("A-1");
    state.lock().unwrap().questions = vec![
        json!({"id": "question-1", "question": "Which change?"}),
        json!({"id": "question-2", "question": "Next question"}),
    ];
    let rows = crate::assistant::rows(fixture.shared(), false).unwrap();
    assert_eq!(rows[0].state, wire::rows::AssistantState::Waiting);
    assert_eq!(rows[0].question_id.as_deref(), Some("question-1"));
    assert_eq!(rows[0].wait.as_ref().unwrap().text, "Which change?");
    let reply: SessionCardAnswered = owner.ok(
        SESSION_CARD_ANSWER,
        json!({
            "id":"A-1", "card":"question-1", "answers":[{"options":["Fix"], "text":"in this worktree"}],
        }),
    );
    assert_eq!(reply.status, "delivered");
    assert_eq!(
        state.lock().unwrap().requests.last().unwrap(),
        &(
            "card_answer".to_owned(),
            json!({
                "card":"question-1", "answers":[{"options":["Fix"], "text":"in this worktree"}],
                "dismiss":false, "device":"local",
            })
        )
    );
    let rows = crate::assistant::rows(fixture.shared(), false).unwrap();
    assert_eq!(rows[0].question_id.as_deref(), Some("question-2"));
    let cancelled: SessionCancelReply = owner.ok(
        SESSION_CANCEL,
        SessionCancel {
            id: first.session,
            turn: first.turn,
        },
    );
    assert!(cancelled.cancelled);
    assert!(!state.lock().unwrap().open);
    owner.ok::<Value>(
        ASSISTANT_SESSION,
        AssistantSessionOp::End {
            session: "A-1".into(),
        },
    );
    assert!(state.lock().unwrap().stopped);
    let second: SessionSent = owner.ok(SESSION_SEND, quick_send("next", "next chat"));
    assert_eq!(second.session, "A-2");
    assert_eq!(second.entry, "A-2/input/next");
    assert_eq!(placement.state("A-2").lock().unwrap().text, ["next chat"]);
    assert!(
        fixture
            .root()
            .join(".corral-next/chat/A-1/pi.jsonl")
            .is_file()
    );
    owner.ok::<()>(SESSION_TERMINATE, json!({"id":"A-2"}));
    let relaunched: SessionRelaunched = owner.ok(SESSION_RELAUNCH, json!({"id":"A-2"}));
    assert_eq!(relaunched.id, "A-2");
    owner.ok::<()>(SESSION_TERMINATE, json!({"id":"A-2"}));

    let sent: wire::session_host::SessionSent = owner.ok(
        wire::session_host::SESSION_SEND,
        json!({"id": "quick", "cmd_id": "app-chat:1", "text": "tagged cold chat", "policy": "steer"}),
    );
    assert_eq!(sent.session, "A-2");
    assert_eq!(sent.entry, "A-2/input/app-chat:1");
    let detail: DetailReply = owner.ok(
        wire::app::DETAIL_GET,
        json!({"kind": "session", "id": "quick"}),
    );
    let Some(DetailBody::Session(page)) = detail.body else {
        panic!("expected quick session detail")
    };
    assert_eq!(page.id, sent.session);
    assert!(page.messages.iter().any(|message| message.id == sent.entry));
    owner.ok::<()>(SESSION_TERMINATE, json!({"id": "A-2"}));
}
