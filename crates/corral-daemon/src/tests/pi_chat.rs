use super::*;
use corral_engine::{LaunchSpec, Runner};
use corral_protocol::{
    assistant::*,
    detail::{DetailBody, DetailReply, SessionItem},
    session::*,
    subscription::SubDelta,
};
use corral_types::{Origin, SessionRef, SessionStatus};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    sync::Mutex,
    thread::{self, JoinHandle},
};

#[derive(Default)]
struct HostState {
    requests: Vec<(String, Value)>,
    text: Vec<String>,
    held: Vec<String>,
    transcript_tail: Option<String>,
    events: Vec<Value>,
    paused: bool,
    open: bool,
    stopped: bool,
    questions: Vec<Value>,
}

impl HostState {
    fn emit(&mut self, text: &str) {
        self.events.push(json!({"seq": self.events.len() + 1, "at": "2026-11-18T12:00:00Z", "kind": "live_text", "turn": 1, "message": 1, "offset": 0, "text": text}));
    }
}

struct Host {
    socket: PathBuf,
    state: Arc<Mutex<HostState>>,
    handle: Option<JoinHandle<()>>,
}

impl Host {
    fn start(socket: PathBuf, id: String, role: &'static str, dir: &Path, record: bool) -> Self {
        let transcript = dir.join("pi.jsonl");
        fs::write(&transcript, concat!(
            "{\"type\":\"session\",\"id\":\"fixture\",\"version\":3}\n",
            "{\"type\":\"message\",\"id\":\"terminal\",\"parentId\":null,\"timestamp\":\"2026-11-18T12:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"terminal input\"}}\n",
            "{\"type\":\"message\",\"id\":\"first\",\"parentId\":\"terminal\",\"timestamp\":\"2026-11-18T12:00:01Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"recorded **Pi** text\"}]}}\n",
        )).unwrap();
        if record {
            fs::write(
                dir.join("session.json"),
                json!({"pid": 1234, "sessionFile": transcript}).to_string(),
            )
            .unwrap();
        }
        let listener = UnixListener::bind(&socket).unwrap();
        let state = Arc::new(Mutex::new(HostState::default()));
        let worker = Arc::clone(&state);
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                if line.is_empty() {
                    if worker.lock().unwrap().stopped {
                        break;
                    }
                    continue;
                }
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["version"], 2);
                let command = request["command"].as_str().unwrap();
                let args = &request["args"];
                let mut state = worker.lock().unwrap();
                if !matches!(
                    command,
                    "hello" | "live_since" | "events_since" | "get_state"
                ) {
                    state.requests.push((command.into(), args.clone()));
                }
                let result = match command {
                    "hello" => {
                        json!({"protocol": 2, "role": role, "session_id": id, "session_file": transcript, "pid": 1234})
                    }
                    "get_state" => {
                        json!({"paused": state.paused, "turn": {"current": 1, "open": state.open}, "owner_questions": state.questions, "last_error": null})
                    }
                    "events_since" => {
                        json!({"last": 0, "next": 0, "events": [], "more": false, "gap": null})
                    }
                    "live_since" => {
                        let last = state.events.len();
                        let seq = args["seq"].as_u64().unwrap() as usize;
                        json!({"last": last, "next": last, "events": state.events.get(seq..).unwrap_or(&[]), "more": false, "gap": null})
                    }
                    "submit_input" => {
                        let text = args["text"].as_str().unwrap().to_owned();
                        if state.paused && args["origin"] == "auto" {
                            state.held.push(text);
                            json!({"request_id":request["id"], "disposition":"queued"})
                        } else {
                            assert_eq!(args["origin"], "owner");
                            if record {
                                let request_id = request["id"].as_str().unwrap();
                                let input_id = format!("owner-{request_id}");
                                let answer_id = format!("answer-{request_id}");
                                let mut file = fs::OpenOptions::new()
                                    .append(true)
                                    .open(&transcript)
                                    .unwrap();
                                let input = json!({
                                    "type": "message", "id": input_id,
                                    "parentId": state.transcript_tail.as_deref().unwrap_or("first"),
                                    "timestamp": "2026-11-18T12:00:02Z",
                                    "message": {"role": "user", "content": [{"type": "text", "text": text}],
                                        "corralRequestId": request_id}
                                });
                                let answer = json!({
                                    "type": "message", "id": answer_id, "parentId": input_id,
                                    "timestamp": "2026-11-18T12:00:03Z",
                                    "message": {"role": "assistant", "content": "assistant live text"}
                                });
                                writeln!(file, "{input}\n{answer}").unwrap();
                                state.transcript_tail = Some(answer_id);
                            }
                            state.open = true;
                            state.text.push(text);
                            state.emit("assistant live text");
                            json!({"request_id":request["id"], "disposition":"started", "turn":1})
                        }
                    }
                    "cancel" => {
                        let cancelled = std::mem::replace(&mut state.open, false);
                        json!({"cancelled": cancelled, "turn": 1})
                    }
                    "pause" => {
                        state.paused = true;
                        let cancelled = std::mem::replace(&mut state.open, false);
                        json!({"paused": true, "cancelled": cancelled})
                    }
                    "resume" => {
                        state.paused = false;
                        let held = std::mem::take(&mut state.held);
                        state.text.extend(held);
                        json!({"paused": false})
                    }
                    "answer" => {
                        state
                            .questions
                            .retain(|question| question["id"] != args["question"]);
                        json!({"status":"delivered"})
                    }
                    "rewind" if args["list"] == true => {
                        json!({"checkpoints": [{"id": "turn-1", "phase": "start", "turn": 1}], "rewinds": []})
                    }
                    "rewind" => {
                        json!({"turn": args["turn"], "checkpoint":"turn-1", "rescue_ref":"refs/worktree/corral/rescue/1", "from_head":"new", "head":"old", "at":"2026-11-18T12:00:00Z"})
                    }
                    "shutdown" => {
                        state.stopped = true;
                        json!({"stopping": true})
                    }
                    _ => panic!("unexpected Pi method: {command}"),
                };
                let reply = json!({"id":request["id"], "version":2, "ok":true, "result":result});
                let _ = writeln!(stream, "{reply}");
                if state.stopped {
                    break;
                }
            }
        });
        Self {
            socket,
            state,
            handle: Some(handle),
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stopped = true;
        let _ = UnixStream::connect(&self.socket);
        self.handle.take().unwrap().join().unwrap();
    }
}

#[derive(Clone, Default)]
struct Placement {
    tasks: FakeRunner,
    hosts: Arc<Mutex<BTreeMap<String, Host>>>,
}

impl Runner for Placement {
    fn start(&self, spec: &LaunchSpec) -> anyhow::Result<SessionRef> {
        self.tasks.start(spec)
    }
    fn status(&self, session: &SessionRef) -> anyhow::Result<SessionStatus> {
        self.tasks.status(session)
    }
    fn send(&self, session: &SessionRef, text: &str, origin: Origin) -> anyhow::Result<()> {
        self.tasks.send(session, text, origin)
    }
    fn stop(&self, session: &SessionRef) -> anyhow::Result<()> {
        if let SessionRef::Process { dir, .. } = session {
            let key = dir.file_name().unwrap().to_str().unwrap();
            let host = self.hosts.lock().unwrap().remove(key).unwrap();
            fs::remove_file(&host.socket).unwrap();
            drop(host);
            Ok(())
        } else {
            self.tasks.stop(session)
        }
    }
    fn focus(&self, session: &SessionRef) -> anyhow::Result<()> {
        self.tasks.focus(session)
    }
    fn start_process(&self, command: Command, dir: PathBuf) -> anyhow::Result<SessionRef> {
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
                .any(|args| args[0] == "--model" && args[1] == "anthropic-api/claude-opus-5-5")
        );
        assert!(
            command.get_envs().any(|(key, value)| key == "CORRAL_ACTOR"
                && value.is_some_and(|value| value == "assistant"))
        );
        let socket = PathBuf::from(args[4]);
        let id = dir.file_name().unwrap().to_str().unwrap().to_owned();
        self.hosts
            .lock()
            .unwrap()
            .insert(id.clone(), Host::start(socket, id, "assistant", &dir, true));
        Ok(SessionRef::Process { pid: 1234, dir })
    }
}

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
        false,
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
    let placement = Placement::default();
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
    assert!(rows[0].id.starts_with("A-1/replay/"));
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
    let state = Arc::clone(&placement.hosts.lock().unwrap()["A-1"].state);
    state.lock().unwrap().questions = vec![
        json!({"id": "question-1", "question": "Which change?"}),
        json!({"id": "question-2", "question": "Next question"}),
    ];
    let rows = crate::assistant::rows(fixture.shared()).unwrap();
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
    let rows = crate::assistant::rows(fixture.shared()).unwrap();
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
    assert_eq!(
        placement.hosts.lock().unwrap()["A-2"]
            .state
            .lock()
            .unwrap()
            .text,
        ["next chat"]
    );
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
}
