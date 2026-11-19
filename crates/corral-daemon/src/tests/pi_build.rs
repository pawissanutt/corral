use std::{
    ffi::OsStr,
    io::Write,
    os::unix::{fs::symlink, process::ExitStatusExt},
    process::{ExitStatus, Output},
    time::Instant,
};

use corral_protocol::{
    build::{BUILD_REMOVE, BUILD_START, BuildRemove, BuildRemoved},
    detail::{DetailBody, DetailReply, HostPresence, SessionPage},
    rows::{AssistantKind, AssistantLocation, AssistantRow, AssistantState},
    session::{SESSION_RELAUNCH, SESSION_TERMINATE, SessionRelaunched},
    session_host::{
        InputDisposition, SESSION_CARD_ANSWER, SESSION_SEND, SessionCardAnswered, SessionSent,
    },
    session_state::InputOrigin,
};
use corral_types::SessionRef;

use crate::{app::dispatch as app_dispatch, conn::Peer, topics::Subscriptions};

use super::{
    fake_host::{AT, Host, PID, Placement, Script},
    *,
};

const MODEL: &str = "anthropic-api/claude-opus-5-5";

fn dir(fixture: &Fixture, id: &str) -> PathBuf {
    fixture.root().join("pi-data/build").join(id)
}

fn save_record(dir: &Path, workspace: &Path, socket: &Path, here: bool) {
    let id = dir.file_name().unwrap().to_str().unwrap();
    let record = json!({
        "version": 1, "id": id, "name": "walk", "workspaceRoot": workspace,
        "repoRoot": workspace.join("repo"), "worktree": workspace.join("repo/.worktrees/build-walk"),
        "project": "demo", "here": here, "branch": "build/walk", "base": "base-commit",
        "created": AT, "updated": AT, "socket": socket, "pid": PID, "sessionFile": dir.join("pi.jsonl"),
        "route": {"provider": "anthropic-api", "model": MODEL, "effort": "high"},
    });
    fs::write(dir.join("session.json"), record.to_string()).unwrap();
}

fn terminal(fixture: &Fixture, id: &str, script: Script) -> Host {
    let dir = dir(fixture, id);
    let socket = fixture.root().join("terminal-build.sock");
    fs::create_dir_all(&dir).unwrap();
    save_record(&dir, fixture.root(), &socket, false);
    Host::start(socket, id.into(), "build", &dir, script)
}

fn argument<'a>(args: &[&'a OsStr], flag: &str) -> Option<&'a OsStr> {
    args.windows(2)
        .find_map(|pair| (pair[0] == flag).then_some(pair[1]))
}

fn launch_fixture() -> (Fixture, Placement) {
    let placement = Placement::new(|command, dir| {
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(command.get_program(), "corral-pi");
        assert_eq!(args[0], "build");
        assert!(args.contains(&OsStr::new("--headless")));
        let workspace = command
            .get_envs()
            .find_map(|(key, value)| {
                (key == "CORRAL_NEXT_ROOT")
                    .then_some(value)
                    .flatten()
                    .map(Path::new)
            })
            .unwrap();
        let checkout = workspace.join("repo");
        assert_eq!(command.get_current_dir(), Some(checkout.as_path()));
        let data_root = workspace.join("pi-data");
        assert_eq!(argument(&args, "--data-dir"), Some(data_root.as_os_str()));
        assert!(Path::new(argument(&args, "--corral").unwrap()).is_absolute());
        let id = dir.file_name().unwrap();
        if dir.join("session.json").is_file() {
            assert_eq!(argument(&args, "--resume"), Some(id));
            assert!(argument(&args, "--id").is_none());
        } else {
            assert_eq!(argument(&args, "--id"), Some(id));
            assert_eq!(argument(&args, "--project"), Some(OsStr::new("demo")));
            assert_eq!(argument(&args, "--model"), Some(OsStr::new(MODEL)));
            assert!(argument(&args, "--resume").is_none());
        }
        let socket = PathBuf::from(argument(&args, "--socket").unwrap());
        save_record(dir, workspace, &socket, false);
        fs::write(dir.join("journal.json"), json!({
            "title": "Journal title", "diff": {"commits": 1, "files": 2, "added": 3, "removed": 4},
        }).to_string()).unwrap();
        let mut script = Script::default();
        script.pages.push_back(json!({"last": 0, "next": 0, "events": [], "more": false, "state": {
            "mode": "headless", "turn": 0, "open": false, "paused": false, "queue_total": 0,
            "model": MODEL, "effort": "high", "usage": {"tokens": 0, "cost_partial": false,
                "context_tokens": 0, "context_window": 0},
            "children": {"live": 0, "waiting": 0, "total": 0}, "jobs": {"live": 0, "waiting": 0, "total": 0},
        }}));
        Some(Host::start(
            socket,
            id.to_str().unwrap().into(),
            "build",
            dir,
            script,
        ))
    });
    let runner = placement.clone();
    let fixture = Fixture::with_runner(Arc::new(move || Box::new(runner.clone())));
    fs::create_dir(fixture.root().join("repo")).unwrap();
    fs::write(
        fixture.root().join(crate::STATE_DIR).join("corral.toml"),
        "[projects.demo]\npath = 'repo'\n",
    )
    .unwrap();
    (fixture, placement)
}

fn rows(socket: &Path, caps: Vec<String>) -> Vec<AssistantRow> {
    let mut subscriber = Client::connect_with(socket, caps);
    subscriber.ok::<()>(wire::SUB_OPEN, json!({"topic": "assistant"}));
    let frame = read_frame(&mut subscriber.0).unwrap().unwrap();
    let notice: rpc::Notification<SubDelta> = serde_json::from_slice(&frame.payload).unwrap();
    let SubDelta::Assistant { delta } = notice.params else {
        panic!("expected assistant rows")
    };
    delta.rows
}

fn page(client: &mut Client, id: &str) -> SessionPage {
    let detail: DetailReply =
        client.ok(wire::app::DETAIL_GET, json!({"kind": "session", "id": id}));
    let Some(DetailBody::Session(page)) = detail.body else {
        panic!("expected session transcript")
    };
    page
}

#[test]
fn every_page_of_an_unchanged_transcript_carries_the_first_pages_etag_and_an_append_changes_it() {
    let fixture = Fixture::new();
    let id = "B-paged";
    let host = terminal(&fixture, id, Script::default());
    let mut transcript = fs::File::create(&host.transcript).unwrap();
    let text = "x".repeat(1600);
    let mut parent = None;
    for index in 0..20 {
        let entry = format!("entry-{index}");
        let message = json!({
            "type": "message", "id": entry, "parentId": parent,
            "message": {"role": "assistant", "content": text},
        });
        writeln!(transcript, "{message}").unwrap();
        parent = Some(entry);
    }
    let mut owner = Client::connect_with(&fixture.unix_server(), vec![wire::BUILD_V1.into()]);
    let first: DetailReply = owner.ok(wire::app::DETAIL_GET, json!({"kind": "session", "id": id}));
    let cursor = first
        .next
        .as_deref()
        .expect("expected a second transcript page");
    let second: DetailReply = owner.ok(
        wire::app::DETAIL_GET,
        json!({"kind": "session", "id": id, "page": cursor}),
    );
    assert!(second.next.is_none());
    assert!(matches!(second.body, Some(DetailBody::Session(page)) if !page.messages.is_empty()));
    assert_eq!(first.etag, second.etag);

    let conditional = json!({"kind": "session", "id": id, "etag": first.etag});
    let unchanged: DetailReply = owner.ok(wire::app::DETAIL_GET, &conditional);
    assert_eq!(unchanged.etag, first.etag);
    assert!(unchanged.body.is_none());
    assert!(unchanged.next.is_none());

    let entry = json!({
        "type": "message", "id": "appended", "parentId": parent,
        "message": {"role": "assistant", "content": "appended entry"},
    });
    writeln!(transcript, "{entry}").unwrap();
    let changed: DetailReply = owner.ok(wire::app::DETAIL_GET, conditional);
    assert!(changed.body.is_some());
    assert_ne!(changed.etag, first.etag);

    let stale = owner
        .call(
            wire::app::DETAIL_GET,
            json!({"kind": "session", "id": id, "page": cursor}),
        )
        .unwrap_err();
    assert_eq!(stale.code, rpc::ErrorCode::Conflict);
    let malformed = owner
        .call(
            wire::app::DETAIL_GET,
            json!({"kind": "session", "id": id, "page": "transcript:not-a-stamp"}),
        )
        .unwrap_err();
    assert_eq!(malformed.code, rpc::ErrorCode::InvalidParams);
}

#[test]
fn a_build_started_by_the_daemon_is_listed_resolved_and_stopped() {
    let (fixture, placement) = launch_fixture();
    let socket = fixture.unix_server();
    let mut owner = Client::connect_with(&socket, vec![wire::BUILD_V1.into()]);
    let row: AssistantRow = owner.ok(
        BUILD_START,
        json!({"project": "demo", "name": "walk", "here": false, "model": MODEL}),
    );
    assert_eq!(row.kind, AssistantKind::Build);
    assert_eq!(row.title, "Journal title");
    assert_eq!(row.state, AssistantState::Ready);
    assert_eq!(row.live_in, AssistantLocation::Daemon);
    assert_eq!(row.model.as_deref(), Some(MODEL));
    let build = row.build.unwrap();
    assert_eq!(build.branch.as_deref(), Some("build/walk"));
    assert_eq!(build.diff.unwrap().files, 2);
    assert!(rows(&socket, vec![wire::ASSISTANT_V1.into()]).is_empty());
    let listed = rows(&socket, vec![wire::BUILD_V1.into()]);
    assert_eq!(listed[0].id, row.id);
    assert_eq!(listed[0].model.as_deref(), Some(MODEL));
    let page_before = page(&mut owner, &row.id);
    assert_eq!(page_before.role, "build");
    assert_eq!(page_before.host, HostPresence::Running);
    let state = placement.state(&row.id);
    owner.ok::<()>(SESSION_TERMINATE, json!({"id": row.id}));
    assert!(state.lock().unwrap().stopped);
    assert_eq!(page(&mut owner, &row.id).host, HostPresence::NotRunning);
    assert!(dir(&fixture, &row.id).join("pi.jsonl").is_file());
    let cold: rpc::Error = owner
        .call(
            SESSION_SEND,
            json!({
                "id": row.id, "text": "do not start it", "cmd_id": "cold:1", "policy": "steer",
            }),
        )
        .unwrap_err();
    assert_eq!(cold.code, rpc::ErrorCode::Refused);
    assert_eq!(page(&mut owner, &row.id).host, HostPresence::NotRunning);
    let resumed: SessionRelaunched = owner.ok(SESSION_RELAUNCH, json!({"id": row.id}));
    assert_eq!(resumed.id, row.id);
    assert_eq!(page(&mut owner, &row.id).messages, page_before.messages);
    owner.ok::<()>(SESSION_TERMINATE, json!({"id": row.id}));
}

#[test]
fn a_terminal_started_build_is_listed_while_its_socket_answers_and_cold_after() {
    let fixture = Fixture::new();
    let mut script = Script::default();
    script.mode = "interactive";
    let host = terminal(&fixture, "B-term", script);
    let live = crate::builds::rows(fixture.shared()).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].id, "B-term");
    assert_eq!(live[0].live_in, AssistantLocation::Pane);
    drop(host);
    let stopped = crate::builds::rows(fixture.shared()).unwrap();
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0].state, AssistantState::Cold);
    assert_eq!(stopped[0].live_in, AssistantLocation::None);
}

#[test]
fn a_build_of_another_workspace_is_not_listed() {
    let fixture = Fixture::new();
    let workspace = fixture.root().join("other-workspace");
    fs::create_dir(&workspace).unwrap();
    let dir = dir(&fixture, "B-other");
    fs::create_dir_all(&dir).unwrap();
    save_record(&dir, &workspace, &fixture.root().join("empty.sock"), false);
    assert!(crate::builds::rows(fixture.shared()).unwrap().is_empty());
    let socket = fixture.unix_server();
    let mut owner = Client::connect_with(&socket, vec![wire::BUILD_V1.into()]);
    let error = owner
        .call(
            wire::app::DETAIL_GET,
            json!({"kind": "session", "id": "B-other"}),
        )
        .unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::NotFound);
    let malformed = owner
        .call(SESSION_RELAUNCH, json!({"id": "B-../outside"}))
        .unwrap_err();
    assert_eq!(malformed.code, rpc::ErrorCode::InvalidParams);
}

#[test]
fn a_card_answer_reaches_the_host_and_its_brief_leaves_the_state() {
    let fixture = Fixture::new();
    let id = "B-card";
    let mut script = Script::default();
    script.state = Some(json!({
        "mode": "headless", "turn": 1, "open": false, "paused": false, "queue_total": 0,
        "usage": {"tokens": 0, "cost_partial": false, "context_tokens": 0, "context_window": 0},
        "children": {"live": 0, "waiting": 0, "total": 0},
        "jobs": {"live": 0, "waiting": 0, "total": 0},
        "cards": [
            {"id": "question-1", "kind": "question", "title": "Which option?"},
            {"id": "question-2", "kind": "question", "title": "Which other option?"},
        ],
    }));
    let host = terminal(&fixture, id, script);
    let socket = fixture.unix_server();
    crate::topics::start_session_live(fixture.shared()).unwrap();
    let caps = || vec![wire::BUILD_V1.into(), wire::app::SESSION_STATE_V1.into()];
    let mut subscriber = Client::connect_with(&socket, caps());
    subscriber.ok::<()>(wire::SUB_OPEN, json!({"topic": "session", "scope": id}));
    let mut snapshot = || {
        let frame = read_frame(&mut subscriber.0).unwrap().unwrap();
        let delta: rpc::Notification<SubDelta> = serde_json::from_slice(&frame.payload).unwrap();
        let SubDelta::Session {
            scope,
            cursor,
            state: Some(state),
            ..
        } = delta.params
        else {
            panic!("expected a state replacement");
        };
        assert_eq!(scope, id);
        subscriber.ok::<()>(
            wire::SUB_ACK,
            json!({"topic": "session", "scope": scope, "cursor": cursor}),
        );
        state
    };
    let before = snapshot();
    assert_eq!(before.cards.len(), 2);
    assert_eq!(before.cards[0].id, "question-1");

    let answers = json!([{"options": ["first"], "text": "the selected option"}]);
    let mut owner = Client::connect_with(&socket, caps());
    let reply: SessionCardAnswered = owner.ok(
        SESSION_CARD_ANSWER,
        json!({"id": id, "card": "question-1", "answers": answers, "dismiss": false}),
    );
    assert_eq!(reply.status, "delivered");
    let remaining = snapshot();
    assert_eq!(remaining.cards.len(), 1);
    assert_eq!(remaining.cards[0].id, "question-2");

    let shared = fixture.shared();
    let devices = shared.devices();
    let addr = "127.0.0.1:7841".parse().unwrap();
    devices.set_listener(addr, &[], None);
    let now = Instant::now();
    let key = [7; 32];
    let offer = devices.pair_begin(now).unwrap();
    devices
        .pairings
        .handshaken(&offer.pair_id, key, "123456".into(), now)
        .unwrap();
    devices
        .pairings
        .claim(&offer.pair_id, "Browser".into(), "test".into(), now)
        .unwrap();
    let paired = devices.pair_confirm(&offer.pair_id).unwrap();
    let (_, guard) = devices
        .attach_key(&key, wire::device::DeviceReach::Loopback)
        .unwrap();

    let (mut reply_stream, mut paired_client) = UnixStream::pair().unwrap();
    app_dispatch::serve(
        &mut reply_stream,
        &Peer::Tcp(addr),
        shared,
        rpc::Request {
            jsonrpc: rpc::JSONRPC.into(),
            id: json!(1),
            method: SESSION_CARD_ANSWER.into(),
            params: json!({"id": id, "card": "question-2", "answers": answers, "dismiss": false}),
        },
        Some(&guard),
        &mut Subscriptions::new(shared),
    )
    .unwrap();
    let frame = read_frame(&mut paired_client).unwrap().unwrap();
    let reply: rpc::Response<SessionCardAnswered> = serde_json::from_slice(&frame.payload).unwrap();
    assert!(matches!(
        reply.outcome,
        rpc::ResponseOutcome::Result { result } if result.status == "delivered"
    ));
    assert_eq!(
        host.state.lock().unwrap().requests.as_slice(),
        &[
            (
                "card_answer".to_owned(),
                json!({
                    "card": "question-1", "answers": answers, "dismiss": false, "device": "local",
                }),
            ),
            (
                "card_answer".to_owned(),
                json!({
                    "card": "question-2", "answers": answers, "dismiss": false, "device": paired.id,
                }),
            ),
        ],
    );
    assert!(snapshot().cards.is_empty());
}

#[test]
fn remove_is_refused_while_the_host_answers() {
    let fixture = Fixture::new();
    let id = "B-live";
    let _host = terminal(&fixture, id, Script::default());
    let request = BuildRemove {
        id: id.into(),
        dry_run: false,
    };
    let refusal = crate::builds::remove_with(fixture.shared(), request.clone(), |_| {
        panic!("prune must not run while the host answers")
    })
    .unwrap_err();
    assert!(refusal.is::<Refusal>());
    let mut owner = Client::connect_with(&fixture.unix_server(), vec![wire::BUILD_V1.into()]);
    let error = owner.call(BUILD_REMOVE, request).unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::Refused);
    assert!(dir(&fixture, id).join("session.json").is_file());
}

#[test]
fn remove_reports_what_prune_kept_and_why() {
    let fixture = Fixture::new();
    let id = "B-cold";
    let build = dir(&fixture, id);
    fs::create_dir_all(&build).unwrap();
    save_record(
        &build,
        fixture.root(),
        &fixture.root().join("absent.sock"),
        false,
    );
    let data_root = fixture.root().join("pi-data");
    let kept = "contains owner input";
    for dry_run in [true, false] {
        let line = if dry_run {
            "would remove checkout"
        } else {
            "removed checkout"
        };
        let removed = crate::builds::remove_with(
            fixture.shared(),
            BuildRemove {
                id: id.into(),
                dry_run,
            },
            |command| {
                assert_eq!(command.get_program(), "corral-pi");
                assert_eq!(command.get_current_dir(), Some(fixture.root()));
                assert!(command.get_envs().any(|(key, value)| {
                    key == "CORRAL_NEXT_ROOT" && value == Some(fixture.root().as_os_str())
                }));
                let mut expected = ["build", "--prune", "--session", id, "--json"]
                    .map(OsStr::new)
                    .to_vec();
                if dry_run {
                    expected.push(OsStr::new("--dry-run"));
                }
                expected.extend([OsStr::new("--data-dir"), data_root.as_os_str()]);
                assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
                Ok(Output {
                    status: ExitStatus::from_raw(0),
                    stdout: serde_json::to_vec(&json!({
                        "worktree": true, "branch": true, "record": false,
                        "kept": kept, "lines": [line],
                    }))
                    .unwrap(),
                    stderr: Vec::new(),
                })
            },
        )
        .unwrap();
        assert_eq!(
            removed,
            BuildRemoved {
                worktree: true,
                branch: true,
                record: false,
                kept: Some(kept.into()),
                lines: vec![line.into()],
            },
        );
        assert!(build.join("session.json").is_file());
    }
}

#[test]
fn a_refused_launch_reports_the_hosts_two_lines_and_leaves_no_directory() {
    let lines = ["refusal from host 42", "resolution from host 42"];
    let placement = Placement::new(move |_, dir| {
        fs::write(dir.join("session.log"), lines.join("\n") + "\n").unwrap();
        None
    });
    let fixture = Fixture::with_runner(Arc::new(move || Box::new(placement.clone())));
    let socket = fixture.unix_server();
    let mut owner = Client::connect_with(&socket, vec![wire::BUILD_V1.into()]);
    let error = owner.call(BUILD_START, json!({"here": false})).unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::Refused);
    assert_eq!(error.message.lines().collect::<Vec<_>>(), lines);
    assert_eq!(
        fs::read_dir(fixture.root().join("pi-data/build"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn an_acknowledged_send_is_served_under_the_acknowledged_id() {
    let fixture = Fixture::new();
    let host = terminal(&fixture, "B-send", Script::default());
    let socket = fixture.unix_server();
    let mut owner = Client::connect_with(
        &socket,
        vec![wire::BUILD_V1.into(), wire::app::SESSION_STATE_V1.into()],
    );
    let ack: SessionSent = owner.ok(
        SESSION_SEND,
        json!({
            "id": "B-send", "cmd_id": "app-id:1", "text": "", "policy": "follow_up", "typed_at": AT,
            "attachments": [{"kind": "link", "url": "https://example.org/notes", "title": "Notes"}],
        }),
    );
    assert_eq!(ack.session, "B-send");
    assert_eq!(ack.entry, "B-send/input/app-id:1");
    assert_eq!(ack.disposition, InputDisposition::Started);
    let served = page(&mut owner, &ack.session);
    let message = served
        .messages
        .iter()
        .find(|message| message.id == ack.entry)
        .unwrap();
    let input = message.input.as_ref().unwrap();
    assert_eq!(input.id.as_deref(), Some("app-id:1"));
    assert_eq!(input.origin, InputOrigin::Owner);
    assert_eq!(input.typed_at.as_deref(), Some(AT));
    assert_eq!(input.device.as_deref(), Some("local"));
    assert_eq!(
        message.attachments[0].url.as_deref(),
        Some("https://example.org/notes")
    );
    assert_eq!(
        host.state.lock().unwrap().requests[0].1["busy_policy"],
        "follow_up"
    );
    let handled: SessionSent = owner.ok(SESSION_SEND, json!({
        "id": "B-send", "cmd_id": "app-id:2", "text": "child input", "policy": "steer", "to": "child-1",
    }));
    assert_eq!(handled.disposition, InputDisposition::Handled);
    assert!(
        !page(&mut owner, &ack.session)
            .messages
            .iter()
            .any(|message| message.id == handled.entry)
    );
}

#[test]
fn a_terminal_typed_message_keeps_its_entry_id() {
    let fixture = Fixture::new();
    let _host = terminal(&fixture, "B-pane", Script::default());
    let socket = fixture.unix_server();
    let mut owner = Client::connect(&socket);
    let served = page(&mut owner, "B-pane");
    let message = &served.messages[0];
    assert_eq!(message.id, "B-pane/entry/terminal");
    let input = message.input.as_ref().unwrap();
    assert_eq!(input.origin, InputOrigin::Pane);
    assert!(input.id.is_none());
}

#[test]
fn a_child_scope_never_reads_outside_its_session_directory() {
    let fixture = Fixture::new();
    let outside = fixture.root().join("private");
    fs::create_dir(&outside).unwrap();
    let external = outside.join("child-1.jsonl");
    fs::write(&external, "{\"type\":\"user\",\"content\":\"private\"}\n").unwrap();
    let returned = external.clone();
    let script = Script::default().reply(
        "child_file",
        move |_, _, _| json!({"session_file": returned}),
    );
    let _host = terminal(&fixture, "B-child", script);
    let dir = dir(&fixture, "B-child");
    fs::write(
        dir.join("journal.json"),
        json!({"children": [{"id": "child-1", "sessionFile": external}]}).to_string(),
    )
    .unwrap();
    symlink(outside, dir.join("children")).unwrap();
    let socket = fixture.unix_server();
    let mut owner = Client::connect(&socket);
    let id = "B-child/child/child-1";
    let rejected = owner
        .call(wire::app::DETAIL_GET, json!({"kind": "session", "id": id}))
        .unwrap_err();
    assert_eq!(rejected.code, rpc::ErrorCode::NotFound);
    fs::remove_file(dir.join("children")).unwrap();
    fs::create_dir(dir.join("children")).unwrap();
    fs::write(
        dir.join("children/child-1.jsonl"),
        "{\"type\":\"user\",\"id\":\"local\",\"content\":\"inside\"}\n",
    )
    .unwrap();
    let child = page(&mut owner, id);
    assert_eq!(child.role, "child");
    assert!(!child.replyable);
    assert_eq!(child.messages[0].id, format!("{id}/entry/local"));
    let refused = owner
        .call(
            SESSION_SEND,
            json!({"id": id, "cmd_id": "child:1", "text": "no", "policy": "steer"}),
        )
        .unwrap_err();
    assert_eq!(refused.code, rpc::ErrorCode::Refused);
}

#[test]
fn a_worker_send_records_the_reply_event() {
    let fixture = Fixture::with_repository();
    let socket = fixture.unix_server();
    let mut owner = Client::connect(&socket);
    let task = owner.new_task("record a worker reply", Vec::new());
    owner.op(dispatch(&task), false);
    let engine = fixture.shared().engine().unwrap();
    let mut run = engine.store.list_runs(&task).unwrap().pop().unwrap();
    let dir = fixture
        .root()
        .join(crate::STATE_DIR)
        .join("runs")
        .join(&task)
        .join(&run.id);
    let id = format!("{task}_{}", run.id);
    let host = Host::start(
        fixture.root().join("worker.sock"),
        id.clone(),
        "worker",
        &dir,
        Script::default(),
    );
    run.session = Some(SessionRef::Pi {
        socket: host.socket.clone(),
        inner: Box::new(run.session.take().unwrap()),
        dir,
    });
    engine.store.update_run(&run).unwrap();
    let ack: SessionSent = owner.ok(SESSION_SEND, json!({
        "id": format!("{task}/{}", run.id), "cmd_id": "worker:1", "text": "continue the work", "policy": "steer",
    }));
    assert_eq!(ack.session, id);
    assert_eq!(ack.entry, format!("{id}/input/worker:1"));
    let events = engine.store.list_events(&task).unwrap();
    assert!(events.iter().any(|event| event.kind == EventKind::Replied
        && event.run.as_deref() == Some(run.id.as_str())
        && event.actor == Actor::Owner
        && event.detail["text"] == "continue the work"));
}
