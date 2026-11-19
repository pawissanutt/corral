use std::{
    fs,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use corral_engine::{Engine, testing::FakeRunner};
use corral_protocol::{
    self as wire,
    config::ConfigDocument,
    frame::{Frame, FrameKind, read_frame, write_frame},
    op::{OpReply, OpSubmit},
    rpc,
    subscription::{SubAck, SubDelta, SubOpen, Topic},
};
use corral_types::{Actor, EventKind, NewTask, Op, Refusal, Status};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{server, state::Shared};

mod fake_host;
mod pi_build;
mod pi_chat;

pub(super) struct Fixture {
    dir: tempfile::TempDir,
    shared: Arc<Shared>,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let runner = FakeRunner::new();
        Self::with_runner(Arc::new(move || Box::new(runner.clone())))
    }

    fn with_runner(runner: crate::state::RunnerFactory) -> Self {
        // Leave headroom under sockaddr_un's path limit and clean up on unwind.
        let dir = tempfile::Builder::new()
            .prefix("corral-d-")
            .tempdir_in("/tmp")
            .unwrap();
        Engine::init(dir.path(), None).unwrap();
        let shared = Arc::new(Shared::new(dir.path().to_path_buf(), Some(runner)).unwrap());
        Self { dir, shared }
    }

    pub(super) fn root(&self) -> &Path {
        self.dir.path()
    }
    pub(super) fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    fn with_repository() -> Self {
        let fixture = Self::new();
        Self::init_repository(&fixture);
        fixture
    }

    fn init_repository(fixture: &Self) {
        let repo = fixture.root().join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        for (key, value) in [
            ("user.name", "Corral Test"),
            ("user.email", "corral@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            git(&repo, &["config", key, value]);
        }
        fs::write(repo.join(".gitignore"), "/.worktrees/\n").unwrap();
        fs::write(repo.join("note"), "base\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "base"]);
        fs::write(
            fixture.root().join(".corral-next/corral.toml"),
            "[projects.demo]\npath = 'repo'\nprofile = 'generic'\ngate = ['test -s note']\n",
        )
        .unwrap();
    }

    pub(super) fn unix_server(&self) -> PathBuf {
        let path = crate::socket_path(self.root());
        server::start(UnixListener::bind(&path).unwrap(), Arc::clone(&self.shared)).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.shared.close();
        crate::topics::join_session_live(&self.shared);
        self.shared.wait_ops();
        self.shared.wait_workers(Duration::from_secs(3));
    }
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Client(UnixStream);
impl Client {
    fn connect(path: &Path) -> Self {
        Self::connect_with(path, vec![wire::TASKS_V1.into()])
    }

    fn connect_with(path: &Path, caps: Vec<String>) -> Self {
        let stream = UnixStream::connect(path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(8)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut client = Self(stream);
        let _: wire::Welcome = client.ok(
            wire::HELLO,
            wire::Hello {
                proto: wire::Proto {
                    major: wire::PROTO_MAJOR,
                    minor: wire::PROTO_MINOR,
                },
                caps,
                client: wire::ClientInfo {
                    kind: "cli".into(),
                    version: "test".into(),
                },
            },
        );
        client
    }

    fn call(&mut self, method: &str, params: impl Serialize) -> Result<Value, rpc::Error> {
        let payload = serde_json::to_vec(&rpc::Request {
            jsonrpc: rpc::JSONRPC.into(),
            id: json!(1),
            method: method.into(),
            params,
        })
        .unwrap();
        write_frame(
            &mut self.0,
            &Frame {
                kind: FrameKind::Control,
                stream: 0,
                payload,
            },
        )
        .unwrap();
        let frame = read_frame(&mut self.0).unwrap().unwrap();
        let reply: rpc::Response = serde_json::from_slice(&frame.payload).unwrap();
        assert_eq!(reply.id, json!(1));
        match reply.outcome {
            rpc::ResponseOutcome::Result { result } => Ok(result),
            rpc::ResponseOutcome::Error { error } => Err(error),
        }
    }

    fn ok<T: DeserializeOwned>(&mut self, method: &str, params: impl Serialize) -> T {
        serde_json::from_value(self.call(method, params).unwrap()).unwrap()
    }

    fn op(&mut self, op: Op, force: bool) -> OpReply {
        self.ok(wire::op::OP_SUBMIT, OpSubmit { op, force })
    }

    fn new_task(&mut self, title: &str, depends_on: Vec<String>) -> String {
        let OpReply::Done { task: Some(id), .. } = self.op(
            Op::New {
                task: NewTask {
                    title: title.into(),
                    project: Some("demo".into()),
                    depends_on,
                    ..NewTask::default()
                },
            },
            false,
        ) else {
            panic!("new operation didn't return a task")
        };
        id
    }
}

fn dispatch(id: &str) -> Op {
    Op::Dispatch {
        id: id.into(),
        profile: None,
        base: None,
    }
}

#[test]
fn socket_dispatch_done_and_approve_land_a_git_change() {
    let fixture = Fixture::with_repository();
    let mut client = Client::connect(&fixture.unix_server());
    let id = client.new_task("land a change", Vec::new());
    assert!(matches!(
        client.op(dispatch(&id), false),
        OpReply::Done { .. }
    ));
    let engine = fixture.shared.engine().unwrap();
    let run = engine.store.list_runs(&id).unwrap().pop().unwrap();
    fs::write(run.worktree.join("note"), "work landed\n").unwrap();
    assert!(matches!(
        client.op(
            Op::Done {
                id: id.clone(),
                summary: "finished".into()
            },
            false
        ),
        OpReply::Done { .. }
    ));
    assert!(matches!(
        client.op(
            Op::Approve {
                id: id.clone(),
                onto: None
            },
            false
        ),
        OpReply::Done { .. }
    ));
    assert_eq!(engine.store.require_task(&id).unwrap().status, Status::Done);
    assert_eq!(
        fs::read_to_string(fixture.root().join("repo/note")).unwrap(),
        "work landed\n"
    );
    assert!(!run.worktree.exists());
}

#[test]
fn socket_review_before_done_is_not_found() {
    use wire::detail::{DetailGet, DetailRef};

    let fixture = Fixture::new();
    let mut client = Client::connect(&fixture.unix_server());
    let id = client.new_task("not submitted for review", Vec::new());
    let error = client
        .call(
            wire::app::DETAIL_GET,
            DetailGet {
                target: DetailRef::Review { task: id },
                page: None,
                etag: None,
            },
        )
        .unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::NotFound);
}

#[test]
fn socket_usage_add_preserves_reported_harness() {
    let fixture = Fixture::new();
    let mut client = Client::connect(&fixture.unix_server());
    let reply: OpReply = client.ok(
        wire::op::OP_SUBMIT,
        json!({
            "op": {"usage_add":{"usage":{
                "model":"model", "at":"2026-02-03T00:00:00Z", "input":1, "output":1,
                "cache_read":0, "cache_write":0, "harness":"reported-harness"
            }}}
        }),
    );
    assert_eq!(reply, OpReply::Done { seq: 0, task: None });
    let engine = fixture.shared.engine().unwrap();
    let rows = engine.store.list_usage().unwrap();
    assert_eq!(rows[0].harness.as_deref(), Some("reported-harness"));
}

#[test]
fn socket_refusal_offers_the_force_that_records_what_it_skipped() {
    let fixture = Fixture::with_repository();
    let mut client = Client::connect(&fixture.unix_server());
    let dependency = client.new_task("dependency", Vec::new());
    let id = client.new_task("card with an open dependency", vec![dependency]);
    let op = dispatch(&id);
    let error = client
        .call(
            wire::op::OP_SUBMIT,
            OpSubmit {
                op: op.clone(),
                force: false,
            },
        )
        .unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::Refused);
    let refusal: Refusal = serde_json::from_value(error.data.unwrap()).unwrap();
    let forced = refusal.next.into_iter().find(|next| next.force).unwrap();
    assert_eq!(forced.op, op);
    client.op(forced.op, forced.force);
    let engine = fixture.shared.engine().unwrap();
    let task = engine.store.require_task(&id).unwrap();
    assert_eq!(task.status, Status::Running);
    let events = engine.store.list_events(&id).unwrap();
    let forced = events
        .iter()
        .find(|event| event.kind == EventKind::Forced)
        .unwrap();
    assert_eq!(forced.actor, Actor::Owner);
    assert_eq!(forced.detail["operation"], "dispatch");
    assert!(!forced.detail["skipped"].as_array().unwrap().is_empty());
}

#[test]
fn config_put_refuses_with_keys_or_syntax_location_without_replacing_file() {
    let fixture = Fixture::new();
    let mut client = Client::connect(&fixture.unix_server());
    let before: ConfigDocument = client.ok(wire::config::CONFIG_GET, wire::config::ConfigGet {});
    let rejected = |client: &mut Client, toml| {
        let error = client
            .call(wire::config::CONFIG_PUT, json!({"toml": toml}))
            .unwrap_err();
        assert_eq!(error.code, rpc::ErrorCode::Refused);
        let refusal: Refusal = serde_json::from_value(error.data.unwrap()).unwrap();
        refusal.problems.unwrap()
    };
    let problems = rejected(
        &mut client,
        "
[limits]
running = 0
[projects.site]
path = '.'
profile = 'absent'
[[routing]]
profile = 'also-absent'
",
    );
    assert_eq!(
        problems
            .iter()
            .map(|problem| problem.key.as_deref())
            .collect::<Vec<_>>(),
        [
            Some("limits.running"),
            Some("projects.site.profile"),
            Some("routing[0].profile")
        ]
    );
    let bad_type = rejected(&mut client, "[[routing]]\nprofile = 1\n");
    assert_eq!(bad_type[0].key.as_deref(), Some("routing[0].profile"));
    let unknown = rejected(&mut client, "[projects.site]\nwrong = 1\n");
    assert_eq!(unknown[0].key.as_deref(), Some("projects.site.wrong"));
    let syntax = rejected(&mut client, "[limits]\nrunning = @\n");
    assert_eq!(
        (syntax[0].key.as_deref(), syntax[0].line, syntax[0].column),
        (None, Some(2), Some(11))
    );
    assert_eq!(
        client.ok::<ConfigDocument>(wire::config::CONFIG_GET, wire::config::ConfigGet {}),
        before
    );
}

fn read_delta(client: &mut Client) -> SubDelta {
    let frame = read_frame(&mut client.0).unwrap().unwrap();
    let notification: rpc::Notification<SubDelta> = serde_json::from_slice(&frame.payload).unwrap();
    assert_eq!(notification.method, wire::SUB_DELTA);
    notification.params
}

fn subscribe(client: &mut Client, topic: Topic) -> SubDelta {
    client.ok::<()>(
        wire::SUB_OPEN,
        SubOpen {
            topic,
            since: None,
            scope: None,
        },
    );
    read_delta(client)
}

#[test]
fn external_config_edit_reloads_engine_and_notifies_settings_and_projects() {
    let fixture = Fixture::new();
    let path = fixture.unix_server();
    let mut settings = Client::connect(&path);
    let mut projects = Client::connect(&path);
    let SubDelta::Settings { delta: old } = subscribe(&mut settings, Topic::Settings) else {
        panic!("expected settings delta");
    };
    let SubDelta::Projects {
        delta: old_projects,
    } = subscribe(&mut projects, Topic::Projects)
    else {
        panic!("expected projects delta");
    };
    settings.ok::<()>(
        wire::SUB_ACK,
        SubAck {
            topic: Topic::Settings,
            scope: None,
            cursor: old.cursor,
        },
    );
    projects.ok::<()>(
        wire::SUB_ACK,
        SubAck {
            topic: Topic::Projects,
            scope: None,
            cursor: old_projects.cursor,
        },
    );

    let shared = Arc::clone(fixture.shared());
    let observer = std::thread::spawn(move || {
        let engine = shared.engine().unwrap();
        let seq = engine.store.latest_seq().unwrap();
        let version = engine.store.data_version().unwrap();
        crate::poll_loop(&shared, &engine.store, seq, version);
    });
    let edit = "[defaults]\nmax_rounds = 7\n[projects.site]\npath = '.'\n";
    fs::write(fixture.root().join(".corral-next/corral.toml"), edit).unwrap();

    let SubDelta::Settings { delta } = read_delta(&mut settings) else {
        panic!("expected settings delta");
    };
    let SubDelta::Projects {
        delta: projects_delta,
    } = read_delta(&mut projects)
    else {
        panic!("expected projects delta");
    };
    let document: ConfigDocument =
        settings.ok(wire::config::CONFIG_GET, wire::config::ConfigGet {});
    assert_ne!(delta.rows[0].revision, old.rows[0].revision);
    assert_eq!(delta.rows[0].revision, document.revision);
    assert_eq!(document.toml, edit);
    assert_eq!(delta.rows[0].config["defaults"]["max_rounds"], 7);
    assert_eq!(projects_delta.rows[0].id, "site");
    assert_eq!(
        projects_delta.rows[0]
            .effective
            .as_ref()
            .unwrap()
            .max_rounds,
        7
    );
    assert_eq!(
        fixture.shared.engine().unwrap().config.defaults.max_rounds,
        7
    );

    fixture.shared.stop();
    observer.join().unwrap();
}

#[test]
fn settings_snapshot_distinguishes_builtin_defaults_from_workspace_values() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root().join(".corral-next/corral.toml"),
        "[limits]\nrunning = 9\n[defaults]\nprofile = 'generic'\n",
    )
    .unwrap();
    let mut client = Client::connect(&fixture.unix_server());
    let SubDelta::Settings { delta } = subscribe(&mut client, Topic::Settings) else {
        panic!("expected settings delta");
    };
    let row = &delta.rows[0];
    assert_eq!(row.config["limits"]["running"], 9);
    assert_eq!(row.config["defaults"]["profile"], "generic");
    let built_in = row.built_in_defaults.as_ref().unwrap();
    assert_eq!(built_in["limits"], json!({"running": 4}));
    assert_eq!(
        built_in["defaults"],
        json!({"base": null, "profile": "omp-sol", "branch_prefix": "corral/",
               "max_rounds": 2, "auto_dispatch": false, "auto_land": false})
    );
    assert_eq!(
        serde_json::to_value(row.project_defaults.as_ref().unwrap()).unwrap(),
        json!({"path": "", "base": null, "gate": [], "reviewer": null, "profile": "omp-sol",
               "running": null, "env": {}, "max_rounds": 2, "branch_prefix": "corral/",
               "auto_dispatch": false, "auto_land": false})
    );
}

#[test]
fn a_nudge_during_sync_requires_a_later_pass() {
    let fixture = Fixture::new();
    let shared = fixture.shared();
    assert!(shared.begin_sync(Duration::ZERO));
    assert!(!shared.nudge(None).completed);
    shared.finish_sync(true, None);
    let ack = shared.nudge(Some(Duration::ZERO));
    assert_eq!((ack.pass_seq, ack.completed), (1, false));
    assert!(shared.begin_sync(Duration::ZERO));
    shared.finish_sync(true, None);
    assert_eq!(shared.nudge(None).pass_seq, 2);
}

#[test]
fn external_engine_commits_reach_socket_subscribers() {
    let fixture = Fixture::new();
    let mut client = Client::connect(&fixture.unix_server());
    let shared = Arc::clone(fixture.shared());
    let (ready, wait) = std::sync::mpsc::sync_channel(0);
    let observer = std::thread::spawn(move || {
        let observer = shared.engine().unwrap();
        let version = observer.store.data_version().unwrap();
        ready.send(()).unwrap();
        crate::poll_loop(&shared, &observer.store, 0, version);
    });
    wait.recv_timeout(Duration::from_secs(3)).unwrap();
    let recv_delta = |client: &mut Client| {
        let SubDelta::Tasks { delta } = read_delta(client) else {
            panic!("expected tasks delta")
        };
        delta
    };
    client.ok::<()>(
        wire::SUB_OPEN,
        SubOpen {
            topic: Topic::Tasks,
            since: None,
            scope: None,
        },
    );
    let first = recv_delta(&mut client);
    assert!(first.rows.is_empty());
    client.ok::<()>(
        wire::SUB_ACK,
        SubAck {
            topic: Topic::Tasks,
            scope: None,
            cursor: first.cursor,
        },
    );
    let task = fixture
        .shared()
        .engine()
        .unwrap()
        .apply(
            Op::New {
                task: NewTask {
                    title: "external edit".into(),
                    ..NewTask::default()
                },
            },
            false,
        )
        .unwrap()
        .task
        .unwrap();
    assert_eq!(recv_delta(&mut client).rows[0].task, task);
    fixture.shared().stop();
    observer.join().unwrap();
}
