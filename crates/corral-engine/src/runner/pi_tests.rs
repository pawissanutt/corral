use std::{
    io::{BufRead, BufReader, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

use super::*;
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
        PiRunner {
            inner: &self.placement,
            socket_dir: Some(self.temp.path()),
        }
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
        let socket = self.socket(dir);
        let command = format!(
            "{} --dir {} --socket {}",
            launch.command,
            quoted_path(dir).unwrap(),
            quoted_path(&socket).unwrap()
        );
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
    let host = fixture.host(
        &launch,
        &launch.dir,
        vec![
            hello(2, "T-042/R7"),
            state(false, true, None, None),
            state(false, true, Some("Which target?"), None),
            state(false, false, None, Some("spending cap reached")),
            expect("pause", json!({}), json!({"paused": true})),
            state(true, false, None, None),
            submit("gate failure", "auto", "queued"),
            submit("continue", "owner", "handled"),
            expect(
                "answer",
                json!({"question": "Q1", "option": "A"}),
                json!({"status": "delivered"}),
            ),
            expect("cancel", json!({}), json!({"cancelled": true})),
            expect("resume", json!({}), json!({"paused": false})),
            Box::new(move |request| {
                assert_eq!(request["command"], "shutdown");
                assert!(placement.stops().is_empty());
                json!({"stopping": true})
            }),
        ],
    );
    let session = runner.start(&launch).unwrap();
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
            excerpt: "spending cap reached".into()
        }
    );
    let client = Client::for_session(&session).unwrap();
    assert_eq!(client.pause().unwrap()["paused"], true);
    assert_eq!(runner.status(&session).unwrap(), SessionStatus::Idle);
    runner.send(&session, "gate failure", Origin::Auto).unwrap();
    runner.send(&session, "continue", Origin::Owner).unwrap();
    assert_eq!(
        client.answer("Q1", Answer::Option("A")).unwrap().status,
        "delivered"
    );
    assert_eq!(client.cancel().unwrap()["cancelled"], true);
    assert_eq!(client.resume().unwrap()["paused"], false);
    runner.focus(&session).unwrap();
    assert_eq!(fixture.placement.focuses(), [inner.as_ref().clone()]);
    runner.stop(&session).unwrap();
    host.join();
    assert_eq!(fixture.placement.stops(), [inner.as_ref().clone()]);
    assert_eq!(runner.status(&session).unwrap(), SessionStatus::Gone);
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
