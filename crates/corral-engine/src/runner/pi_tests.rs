use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{ffi::OsStrExt, fs::PermissionsExt, net::UnixListener},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::pi::{BuildLaunch, Client, Disposition, FRAME_MAX, PiRunner, quoted_path};
use super::{InputTag, LaunchSpec, Origin, Runner, SessionRef, SessionStatus};
use crate::runner::{FakeRunner, tests::spec};

type Handler = Box<dyn FnOnce(Value) -> Value + Send>;

fn expect(command: &str, args: Value, result: Value) -> Handler {
    let command = command.to_owned();
    Box::new(move |request| {
        assert_eq!(request["command"], command);
        assert_eq!(request["args"], args);
        result
    })
}

fn serve(socket: &Path, handlers: Vec<Handler>) -> JoinHandle<()> {
    let listener = UnixListener::bind(socket).unwrap();
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600)).unwrap();
    thread::spawn(move || {
        for respond in handlers {
            let (mut connection, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&connection).read_line(&mut line).unwrap();
            assert!(line.len() <= FRAME_MAX + 1 && line.ends_with('\n'));
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["version"], 2);
            assert_eq!(request.as_object().unwrap().len(), 4); // No secret or generation.
            let id = request["id"].clone();
            let result = respond(request);
            serde_json::to_writer(
                &mut connection,
                &json!({"id": id, "version": 2, "ok": true, "result": result}),
            )
            .unwrap();
            connection.write_all(b"\n").unwrap();
        }
    })
}

fn hello(protocol: u32, session_id: &str) -> Handler {
    expect(
        "hello",
        json!({}),
        json!({"protocol": protocol, "role": "worker", "session_id": session_id,
               "session_file": "transcript.jsonl", "pid": 1234}),
    )
}

fn state(paused: bool, open: bool, question: Option<&str>, last_error: Option<&str>) -> Handler {
    let questions: Vec<_> = question
        .into_iter()
        .map(|text| json!({"id": "Q1", "question": text, "status": "pending"}))
        .collect();
    expect(
        "get_state",
        json!({}),
        json!({"paused": paused, "turn": {"current": 2, "open": open},
               "owner_questions": questions, "last_error": last_error}),
    )
}

fn submit(text: &str, origin: &str, disposition: &str) -> Handler {
    expect(
        "submit_input",
        json!({"text": text, "origin": origin, "busy_policy": "steer"}),
        json!({"request_id": "input-1", "disposition": disposition}),
    )
}

struct Fixture {
    temp: tempfile::TempDir,
    placement: FakeRunner,
}

impl Fixture {
    fn new() -> Self {
        // Keep sockets inside the worktree but close to its root for sockaddr_un.
        let worktree = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap();
        let temp = tempfile::Builder::new()
            .prefix("pi-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(worktree)
            .unwrap();
        let placement = FakeRunner::new();
        placement.set_default_status(SessionStatus::Working);
        Self { temp, placement }
    }

    fn runner(&self) -> PiRunner<'_> {
        PiRunner::new(&self.placement).in_test_socket_dir(self.temp.path())
    }

    fn launch(&self, run: &str, role: &str) -> LaunchSpec {
        let mut launch = spec(
            self.temp.path(),
            "corral-pi run --prompt-file '/run dir/prompt.md' --corral '/bin/corral next'",
        );
        launch.dir.set_file_name(run);
        launch.env.push(("CORRAL_ACTOR".into(), role.into()));
        launch
    }

    fn socket(&self, dir: &Path) -> PathBuf {
        let hash = format!("{:x}", Sha256::digest(dir.as_os_str().as_bytes()));
        self.temp.path().join(format!("{}.sock", &hash[..16]))
    }

    fn host(&self, launch: &LaunchSpec, dir: &Path, handlers: Vec<Handler>) -> Host {
        self.host_with_command(launch, dir, handlers, |command| command)
    }

    fn host_with_command(
        &self,
        launch: &LaunchSpec,
        dir: &Path,
        handlers: Vec<Handler>,
        transform: impl FnOnce(String) -> String,
    ) -> Host {
        let socket = self.socket(dir);
        let command = transform(format!(
            "{} --dir {} --socket {}",
            launch.command,
            quoted_path(dir).unwrap(),
            quoted_path(&socket).unwrap()
        ));
        let run_dir = launch.dir.clone();
        let handlers = Arc::new(Mutex::new(Some(handlers)));
        let host = Host::default();
        let handle = host.0.clone();
        self.placement.set_start_hook(move |launch| {
            assert_eq!(launch.command, command);
            assert_eq!(launch.dir, run_dir);
            *handle.lock().unwrap() =
                Some(serve(&socket, handlers.lock().unwrap().take().unwrap()));
            Ok(())
        });
        host
    }
}

#[derive(Default)]
struct Host(Arc<Mutex<Option<JoinHandle<()>>>>);

impl Host {
    fn join(self) {
        self.0.lock().unwrap().take().unwrap().join().unwrap();
    }
}

#[test]
fn pi_worker_lifecycle_and_controls_use_the_socket() {
    let fixture = Fixture::new();
    let launch = fixture.launch("R7 ' worker", "agent");
    let runner = fixture.runner();
    let placement = fixture.placement.clone();
    let host = fixture.host_with_command(
        &launch,
        &launch.dir,
        vec![
            hello(2, "T-042/R7"),
            state(false, true, None, None),
            state(false, true, Some("Which target?"), None),
            state(false, false, None, Some("provider unavailable")),
            expect("pause", json!({}), json!({"paused": true})),
            state(true, false, None, None),
            submit("gate failure", "auto", "queued"),
            submit("continue", "owner", "handled"),
            expect("cancel", json!({}), json!({"cancelled": true})),
            expect("resume", json!({}), json!({"paused": false})),
            Box::new(move |request| {
                assert_eq!(request["command"], "shutdown");
                assert!(placement.stops().is_empty());
                json!({"stopping": true})
            }),
        ],
        |command| {
            format!(
                "env FOO=1 sh -c {}",
                crate::config::shell_quote(&command).unwrap()
            )
        },
    );
    let session = runner
        .start_with(&launch, |spec| {
            spec.command = crate::config::render_wrap(
                "env FOO=1 sh -c {command}",
                &spec.command,
                fixture.temp.path(),
                &spec.cwd,
                &spec.dir,
            )?;
            Ok(())
        })
        .unwrap();
    let SessionRef::Pi { socket, inner, dir } = &session else {
        panic!("expected Pi session")
    };
    assert_eq!(dir, &launch.dir);
    assert_eq!(socket, &fixture.socket(dir));
    assert_eq!(
        fs::metadata(dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(runner.status(&session).unwrap(), SessionStatus::Working);
    assert_eq!(
        runner.status(&session).unwrap(),
        SessionStatus::Blocked {
            excerpt: "Which target?".into()
        }
    );
    assert_eq!(
        runner.status(&session).unwrap(),
        SessionStatus::Blocked {
            excerpt: "provider unavailable".into()
        }
    );
    let client = Client::for_session(&session).unwrap();
    assert_eq!(client.pause().unwrap()["paused"], true);
    assert_eq!(runner.status(&session).unwrap(), SessionStatus::Idle);
    runner.send(&session, "gate failure", Origin::Auto).unwrap();
    runner.send(&session, "continue", Origin::Owner).unwrap();
    assert_eq!(client.cancel().unwrap()["cancelled"], true);
    assert_eq!(client.resume().unwrap()["paused"], false);
    runner.focus(&session).unwrap();
    assert_eq!(fixture.placement.focuses(), [inner.as_ref().clone()]);
    runner.stop(&session).unwrap();
    host.join();
    assert_eq!(fixture.placement.stops(), [inner.as_ref().clone()]);
    assert_eq!(runner.status(&session).unwrap(), SessionStatus::Gone);
}

#[cfg(target_os = "linux")]
#[test]
fn lost_task_host_is_not_kept_running_by_its_idle_placement() {
    let fixture = Fixture::new();
    let dir = fixture.temp.path().join("T-11/R1");
    fs::create_dir_all(&dir).unwrap();
    let socket = fixture.socket(&dir);
    let inner = fixture.placement.start(&spec(&dir, "test-worker")).unwrap();
    fixture.placement.set_status(&inner, SessionStatus::Idle);
    let session = SessionRef::Pi {
        socket: socket.clone(),
        dir: dir.clone(),
        inner: Box::new(inner),
    };
    let runner = PiRunner::new(&fixture.placement);
    for (pid, endpoint, expected) in [
        (std::process::id(), socket.clone(), SessionStatus::Idle),
        (i32::MAX as u32, dir.join("wrong.sock"), SessionStatus::Idle),
        (i32::MAX as u32, socket.clone(), SessionStatus::Gone),
    ] {
        fs::write(
            dir.join("session.json"),
            json!({"id":"T-11/R1", "role":"worker", "pid":pid, "socket":endpoint}).to_string(),
        )
        .unwrap();
        assert_eq!(runner.status(&session).unwrap(), expected);
    }
}

#[test]
fn oversized_legacy_status_proves_liveness_without_claiming_idle_or_gone() {
    let fixture = Fixture::new();
    let socket = fixture.temp.path().join("legacy.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let host = thread::spawn(move || {
        for code in ["response_too_large", "response_too_large", "starting"] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["command"], "get_state");
            let reply = json!({"version": 2, "id": request["id"], "ok": false,
                "error": {"code": code, "reason": "legacy refusal"}});
            writeln!(stream, "{reply}").unwrap();
        }
    });
    let dir = fixture.temp.path().to_owned();
    let session = SessionRef::Pi {
        socket,
        dir: dir.clone(),
        inner: Box::new(SessionRef::Process { dir, pid: 1234 }),
    };
    let runner = fixture.runner();
    assert!(runner.is_live(&session).unwrap());
    assert!(runner.status(&session).is_err());
    assert!(runner.is_live(&session).is_err());
    host.join().unwrap();
}

#[test]
fn pi_protocol_mismatch_stops_the_placement() {
    let fixture = Fixture::new();
    let runner = fixture.runner();
    let launch = fixture.launch("R1", "agent");
    let host = fixture.host(
        &launch,
        &launch.dir,
        vec![expect("hello", json!({}), json!({"protocol": 1}))],
    );
    let error = runner.start(&launch).unwrap_err();
    assert!(format!("{error:#}").contains("reinstall corral-pi"));
    host.join();
    assert_eq!(fixture.placement.stops().len(), 1);
}

#[test]
fn pi_reviewer_restarts_in_the_same_task_directory() {
    let fixture = Fixture::new();
    let runner = fixture.runner();
    let first = fixture.launch("R2", "reviewer");
    let dir = first.dir.with_file_name("reviewer");
    let transcript = dir.join("transcript.jsonl");
    let host = fixture.host(&first, &dir, vec![hello(2, "T-042/reviewer")]);
    let initial = runner.start(&first).unwrap();
    host.join();
    // A dead host need not unlink its socket. Placement still decides liveness.
    assert_eq!(runner.status(&initial).unwrap(), SessionStatus::Working);
    runner.stop(&initial).unwrap();
    fs::remove_file(fixture.socket(&dir)).unwrap();
    fs::write(&transcript, "previous review").unwrap();

    let second = fixture.launch("R3", "reviewer");
    let host = fixture.host(&second, &dir, vec![hello(2, "T-042/reviewer")]);
    let restarted = runner.start(&second).unwrap();
    host.join();
    let SessionRef::Pi {
        dir: restarted_dir, ..
    } = restarted
    else {
        panic!("expected Pi session")
    };
    assert_eq!(restarted_dir, dir);
    assert_eq!(fs::read_to_string(transcript).unwrap(), "previous review");
}

#[test]
fn pi_spills_text_that_exceeds_the_serialized_frame_limit() {
    let fixture = Fixture::new();
    let runner = fixture.runner();
    let launch = fixture.launch("R1", "agent");
    let dir = launch.dir.clone();
    let text = Arc::new("\n".repeat(FRAME_MAX / 2));
    let full_text = text.clone();
    let host = fixture.host(
        &launch,
        &launch.dir,
        vec![
            hello(2, "T-042/R1"),
            Box::new(move |request| {
                assert_eq!(request["command"], "submit_input");
                let path = request["args"]["text"]
                    .as_str()
                    .unwrap()
                    .strip_prefix("Read ")
                    .unwrap()
                    .strip_suffix(" for the full message.")
                    .unwrap();
                let path = Path::new(path);
                assert_eq!(path.parent().unwrap(), dir);
                assert_eq!(fs::read_to_string(path).unwrap(), *full_text);
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
                json!({"request_id": "input-1", "disposition": "started"})
            }),
        ],
    );
    let session = runner.start(&launch).unwrap();
    runner.send(&session, &text, Origin::Owner).unwrap();
    host.join();
}

#[test]
fn pi_spills_expanded_attachment_metadata_and_removes_it_after_each_submission() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let socket = dir.path().join("pi.sock");
    let digest = "0".repeat(64);
    let refs: Vec<_> = (1..=180)
        .map(|i| json!({"kind": "upload", "upload": format!("phone:{i}"), "sha256": digest}))
        .collect();
    let attachments: Vec<_> = (1..=refs.len())
        .map(|i| {
            json!({"id": format!("a{i}"), "kind": "file", "name": "n".repeat(160),
            "media_type": "t".repeat(160), "bytes": 1, "sha256": digest})
        })
        .collect();
    assert!(serde_json::to_vec(&refs).unwrap().len() < 60 * 1024);
    let expanded = serde_json::to_vec(&attachments).unwrap();
    assert!(expanded.len() > FRAME_MAX);
    let opened = Arc::new(Mutex::new(Vec::new()));
    let respond = |disposition: &'static str| -> Handler {
        let dir = dir.path().to_owned();
        let expanded = expanded.clone();
        let opened = Arc::clone(&opened);
        Box::new(move |request| {
            assert_eq!(request["command"], "submit_input_manifest");
            let args = request["args"].as_object().unwrap();
            assert!(!args.contains_key("attachments"));
            assert_eq!(args["text"], ""); // Attachment-only input stays attachment-only.
            assert_eq!(args["origin"], "owner");
            assert_eq!(args["busy_policy"], "follow_up");
            assert_eq!(args["input_id"], "phone:input");
            assert_eq!(args["to"], "parent");
            assert_eq!(args["typed_at"], "2026-11-18T12:00:00Z");
            assert_eq!(args["device"], "phone");
            let meta = &args["attachments_manifest"];
            let name = meta["name"].as_str().unwrap();
            assert_eq!(Path::new(name).file_name().unwrap(), name);
            let path = dir.join(name);
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(fs::read(&path).unwrap(), expanded);
            assert_eq!(meta["bytes"], expanded.len());
            assert_eq!(meta["sha256"], format!("{:x}", Sha256::digest(&expanded)));
            opened.lock().unwrap().push(path);
            json!({"request_id": request["id"], "input": "phone:input", "disposition": disposition})
        })
    };
    let host = serve(&socket, vec![respond("rejected"), respond("queued")]);
    let client = Client::with_dir(&socket, dir.path());
    let tag = InputTag {
        input_id: "phone:input",
        to: Some("parent"),
        typed_at: Some("2026-11-18T12:00:00Z"),
        device: Some("phone"),
        follow_up: true,
        attachments: &attachments,
    };
    assert!(client.submit_input_tagged("", Origin::Owner, tag).is_err());
    assert!(!opened.lock().unwrap()[0].exists());
    assert_eq!(
        client
            .submit_input_tagged("", Origin::Owner, tag)
            .unwrap()
            .disposition,
        Disposition::Queued
    );
    host.join().unwrap();
    let paths = opened.lock().unwrap();
    assert_ne!(paths[0], paths[1]);
    assert!(!paths[1].exists());
}

#[test]
fn a_tagged_reply_keeps_the_owners_input_id() {
    let fixture = Fixture::new();
    let launch = fixture.launch("R1", "agent");
    let runner = fixture.runner();
    let host = fixture.host(
        &launch,
        &launch.dir,
        vec![
            hello(2, "T-042/R1"),
            Box::new(|request| {
                assert_eq!(request["command"], "submit_input");
                assert_ne!(request["id"], "app-test:1");
                assert_eq!(
                    request["args"],
                    json!({
                        "input_id": "app-test:1", "origin": "owner", "text": "continue",
                        "busy_policy": "follow_up", "device": "phone",
                        "typed_at": "2026-11-18T12:00:00Z",
                    })
                );
                json!({"request_id": request["id"], "input": "app-test:1", "disposition": "queued"})
            }),
        ],
    );
    let session = runner.start(&launch).unwrap();
    let reply = runner
        .send_tagged(
            &session,
            "continue",
            Origin::Owner,
            InputTag {
                input_id: "app-test:1",
                to: None,
                typed_at: Some("2026-11-18T12:00:00Z"),
                device: Some("phone"),
                follow_up: true,
                attachments: &[],
            },
        )
        .unwrap();
    assert_eq!(reply.input.as_deref(), Some("app-test:1"));
    assert_eq!(reply.disposition, Disposition::Queued);
    host.join();
}

#[test]
fn board_delivery_retains_identity_after_an_unusable_acknowledgement() {
    for (disposition, wrong_input, rejected) in [
        ("malformed", false, false),
        ("rejected", true, false),
        ("rejected", false, true),
    ] {
        let fixture = Fixture::new();
        let socket = fixture.temp.path().join("board.sock");
        let sent = Arc::new(Mutex::new(None));
        let saved = sent.clone();
        let host = serve(
            &socket,
            vec![
                expect(
                    "get_state",
                    json!({}),
                    json!({
                        "paused": false, "turn": { "open": false }, "owner_questions": [],
                        "activity": { "settled": true, "busy": false, "waiting": false, "turn": 7, "inputs": {} },
                    }),
                ),
                Box::new(move |request| {
                    assert_eq!(request["command"], "submit_input");
                    let input = request["args"]["input_id"].as_str().unwrap();
                    *saved.lock().unwrap() = Some(input.to_owned());
                    // An invalid result, or a rejection for another input, cannot prove failure.
                    json!({"request_id": request["id"], "disposition": disposition,
                           "input": if wrong_input { "board-other" } else { input }})
                }),
            ],
        );
        let pickup = fixture
            .runner()
            .send_obligation(
                &SessionRef::Pi {
                    socket,
                    dir: fixture.temp.path().to_owned(),
                    inner: Box::new(SessionRef::Herdr {
                        workspace: "fake".into(),
                        pane: "fake".into(),
                    }),
                },
                "feedback",
                Origin::Auto,
            )
            .unwrap()
            .unwrap();
        host.join().unwrap();
        assert_eq!(Some(pickup.input_id), *sent.lock().unwrap());
        assert_eq!(pickup.before_turn, Some(7));
        assert_eq!(pickup.rejected, rejected);
    }
}

#[derive(Default)]
struct BuildPlacement {
    started: Mutex<Option<(Command, JoinHandle<()>)>>,
    tasks: FakeRunner,
}

impl Runner for BuildPlacement {
    fn start(&self, spec: &LaunchSpec) -> Result<SessionRef> {
        self.tasks.start(spec)
    }

    fn start_process(&self, command: Command, dir: PathBuf) -> Result<SessionRef> {
        let mut args = command.get_args();
        let socket = args
            .find(|arg| *arg == "--socket")
            .and_then(|_| args.next())
            .unwrap();
        let host = serve(
            Path::new(socket),
            vec![expect(
                "hello",
                json!({}),
                json!({"protocol": 2, "role": "build", "session_id": "B-012",
                "session_file": "transcript.jsonl", "pid": 1234}),
            )],
        );
        *self.started.lock().unwrap() = Some((command, host));
        Ok(SessionRef::Process { pid: 1234, dir })
    }

    fn status(&self, session: &SessionRef) -> Result<SessionStatus> {
        self.tasks.status(session)
    }

    fn send(&self, session: &SessionRef, text: &str, origin: Origin) -> Result<()> {
        self.tasks.send(session, text, origin)
    }

    fn stop(&self, session: &SessionRef) -> Result<()> {
        self.tasks.stop(session)
    }

    fn focus(&self, session: &SessionRef) -> Result<()> {
        self.tasks.focus(session)
    }
}

#[test]
fn build_launch_passes_id_socket_data_dir_and_model() {
    let fixture = Fixture::new();
    let workspace = fixture.temp.path();
    let data_root = workspace.join("pi-data");
    let cwd = workspace.join("project");
    fs::create_dir_all(data_root.join("build")).unwrap();
    fs::create_dir(&cwd).unwrap();
    let placement = BuildPlacement::default();
    let (session, hello) = PiRunner::new(&placement)
        .start_build(BuildLaunch {
            workspace,
            cwd: &cwd,
            data_root: &data_root,
            socket_root: Some(workspace),
            id: "B-012",
            resume: false,
            project: Some("demo"),
            name: Some("walk"),
            here: true,
            profile: Some("api"),
            model: Some("anthropic-api/claude-opus-5-5"),
            effort: Some("high"),
            fallback: &["openai/gpt-5.4".into()],
        })
        .unwrap();
    assert_eq!(hello.session_id, "B-012");
    let SessionRef::Pi { socket, dir, .. } = session else {
        panic!("expected Pi placement")
    };
    assert_eq!(dir, data_root.join("build/B-012"));
    let (command, host) = placement.started.lock().unwrap().take().unwrap();
    host.join().unwrap();
    assert_eq!(command.get_program(), "corral-pi");
    assert_eq!(command.get_current_dir(), Some(cwd.as_path()));
    assert!(
        command.get_envs().any(|(key, value)| {
            key == "CORRAL_NEXT_ROOT" && value == Some(workspace.as_os_str())
        })
    );
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args[0], "build");
    assert!(args.contains(&std::ffi::OsStr::new("--headless")));
    for (flag, value) in [
        ("--id", std::ffi::OsStr::new("B-012")),
        ("--socket", socket.as_os_str()),
        ("--data-dir", data_root.as_os_str()),
        (
            "--model",
            std::ffi::OsStr::new("anthropic-api/claude-opus-5-5"),
        ),
        ("--fallback", std::ffi::OsStr::new("openai/gpt-5.4")),
    ] {
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == flag && pair[1] == value)
        );
    }
}
