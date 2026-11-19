use super::fake_host::{Host, PID, Placement, Script};
use super::*;
use corral_protocol::{
    assistant::*,
    detail::{DetailBody, DetailReply, SessionItem},
    session::*,
    subscription::SubDelta,
};
use corral_types::SessionRef;

fn watch(path: &Path, id: &str) -> Client {
    let mut subscriber = Client::connect(path);
    subscriber.ok::<()>(wire::SUB_OPEN, json!({"topic":"session", "scope":id}));
    subscriber
}

fn text(subscriber: &mut Client, expected: &str) {
    loop {
        let frame = read_frame(&mut subscriber.0).unwrap().unwrap();
        let notification: rpc::Notification<SubDelta> =
            serde_json::from_slice(&frame.payload).unwrap();
        let SubDelta::Session {
            events,
            scope,
            cursor,
            ..
        } = notification.params
        else {
            panic!("expected session page")
        };
        if events.iter().any(|event| matches!(&event.kind, wire::subscription::SessionLiveKind::LiveText { text, .. } if text == expected)) {
            return;
        }
        subscriber.ok::<()>(
            wire::SUB_ACK,
            json!({"topic":"session", "scope":scope, "cursor":cursor}),
        );
    }
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

fn quick_send(cmd: &str, text: &str) -> AssistantSend {
    AssistantSend {
        cmd_id: cmd.into(),
        thread: QUICK_THREAD.into(),
        target: AskTarget::Assistant,
        text: text.into(),
        typed_at: "2026-11-18T12:00:00Z".into(),
        context: Vec::new(),
        attachments: Vec::new(),
    }
}

#[test]
fn assistant_thread_replays_owner_input_with_acknowledged_id() {
    let (fixture, _placement) = chat_fixture();
    let path = fixture.unix_server();
    let mut owner = Client::connect(&path);
    let ack: SendAck = owner.ok(ASSISTANT_SEND, quick_send("send", "app input"));
    let read_thread = || {
        let mut subscriber = Client::connect(&path);
        subscriber.ok::<()>(wire::SUB_OPEN, json!({"topic":"thread", "scope":"A-1"}));
        let frame = read_frame(&mut subscriber.0).unwrap().unwrap();
        let notification: rpc::Notification<SubDelta> =
            serde_json::from_slice(&frame.payload).unwrap();
        let SubDelta::Thread { delta, .. } = notification.params else {
            panic!("expected thread page");
        };
        delta.rows
    };

    let rows = read_thread();
    let texts: Vec<_> = rows.iter().map(|row| row.text.as_deref()).collect();
    assert_eq!(
        texts,
        [
            Some("terminal input"),
            Some("recorded **Pi** text"),
            Some("app input"),
            Some("assistant live text"),
        ],
    );
    assert_eq!(rows[0].id, "A-1/entry/terminal");
    assert_eq!(rows[2].id, ack.entry);
    assert_eq!(rows[2].state, Some(MessageState::Delivered));
    assert_eq!(read_thread(), rows);
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
    let first: SendAck = owner.ok(ASSISTANT_SEND, quick_send("first", "first chat"));
    assert!(first.entry.starts_with("A-1/"));
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
    let reply: SessionPromptAnswered = owner.ok(PROMPT_ANSWER, json!({"session":"A-1", "question":"question-1", "option":"Fix", "text":"in this worktree"}));
    assert_eq!(reply.status, "delivered");
    assert_eq!(
        state.lock().unwrap().requests.last().unwrap(),
        &(
            "answer".into(),
            json!({"question":"question-1", "option":"Fix", "text":"in this worktree"})
        )
    );
    let rows = crate::assistant::rows(fixture.shared(), false).unwrap();
    assert_eq!(rows[0].question_id.as_deref(), Some("question-2"));
    let cancelled: CancelAck = owner.ok(
        ASSISTANT_CANCEL,
        AssistantCancel {
            cmd_id: "first".into(),
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
    let second: SendAck = owner.ok(ASSISTANT_SEND, quick_send("next", "next chat"));
    assert!(second.entry.starts_with("A-2/"));
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
