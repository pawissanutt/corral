use super::*;
use crate::tests::Fixture;
use corral_core::{Actor, Status};
use corral_protocol::{
    self as wire,
    cmd::{
        ActorName, CMD_RESULT, CMD_SUBMIT, CMD_V1, ClientBuild, ClientKind, Via, task::TaskCommand,
    },
    frame::{read_frame, write_frame},
    rpc,
};
use serde::Serialize;
use std::os::unix::net::UnixListener;
use std::thread::JoinHandle;

/// A started command service behind the real connection handler, reached by a raw client.
struct Connection {
    stream: UnixStream,
    server: JoinHandle<Result<()>>,
    shared: Arc<Shared>,
}

impl Connection {
    fn open(fixture: &Fixture) -> Self {
        let shared = Arc::new(Shared::new());
        start(&fixture.ctx, &shared).unwrap();
        let socket = fixture.root.join("cmd-test.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server_shared = Arc::clone(&shared);
        let store = fixture.ctx.store.dir().display().to_string();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            crate::server::serve(stream, server_shared, &store)
        });
        let stream = UnixStream::connect(&socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let mut connection = Self {
            stream,
            server,
            shared,
        };
        let welcome: wire::Welcome = serde_json::from_value(
            connection
                .call(
                    wire::HELLO,
                    wire::Hello {
                        proto: wire::Proto {
                            major: wire::PROTO_MAJOR,
                            minor: wire::PROTO_MINOR,
                        },
                        caps: vec![CMD_V1.into()],
                        client: wire::ClientInfo {
                            kind: "test".into(),
                            version: "0".into(),
                        },
                    },
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(welcome.caps, [CMD_V1]);
        connection
    }

    fn call(&mut self, method: &str, params: impl Serialize) -> Result<Value, rpc::Error> {
        let request = rpc::Request {
            jsonrpc: rpc::JSONRPC.into(),
            id: Value::from(1),
            method: method.into(),
            params,
        };
        write_frame(
            &mut self.stream,
            &Frame {
                kind: FrameKind::Control,
                stream: 0,
                payload: serde_json::to_vec(&request).unwrap(),
            },
        )
        .unwrap();
        let frame = read_frame(&mut self.stream).unwrap().unwrap();
        assert_eq!(frame.kind, FrameKind::Control);
        let response: rpc::Response<Value> = serde_json::from_slice(&frame.payload).unwrap();
        match response.outcome {
            rpc::ResponseOutcome::Result { result } => Ok(result),
            rpc::ResponseOutcome::Error { error } => Err(error),
        }
    }

    /// A `CmdResult` and the output its DATA frames carried.
    fn command(
        &mut self,
        method: &str,
        params: impl Serialize,
    ) -> Result<(CmdResult, Option<CmdOutput>), rpc::Error> {
        let result: CmdResult = serde_json::from_value(self.call(method, params)?).unwrap();
        let output = result.output.as_ref().map(|body| {
            let mut bytes = Vec::new();
            for _ in 0..body.frames {
                let frame = read_frame(&mut self.stream).unwrap().unwrap();
                assert_eq!(frame.kind, FrameKind::Data);
                assert_eq!(frame.stream, body.stream);
                bytes.extend(frame.payload);
            }
            assert_eq!(bytes.len() as u64, body.bytes);
            serde_json::from_slice(&bytes).unwrap()
        });
        Ok((result, output))
    }

    fn submit(&mut self, submit: &CmdSubmit) -> Result<(CmdResult, Option<CmdOutput>), rpc::Error> {
        self.command(CMD_SUBMIT, submit)
    }

    fn finish(mut self) {
        self.call(wire::BYE, ()).unwrap();
        self.server.join().unwrap().unwrap();
        self.shared.close();
    }
}

fn request(fixture: &Fixture, cmd_id: &str, command: Command) -> CmdSubmit {
    corral_ops::pm_read::set_build_identity("test");
    let (exe, build) = corral_ops::pm_read::client_identity().unwrap();
    let issued = Timestamp::now();
    CmdSubmit {
        cmd_id: cmd_id.into(),
        caller: Caller {
            principal: Principal::Unverified,
            client: ClientKind::Cli,
            actor: None,
            via: Via::Cli,
            assistant: None,
            planning_safe: false,
            cwd: Some(fixture.root.display().to_string()),
            env: CallerEnv::default(),
        },
        client_build: Some(ClientBuild { exe, build }),
        expect: None,
        issued_at: issued.to_string(),
        expires_at: issued
            .checked_add(SignedDuration::from_secs(120))
            .unwrap()
            .to_string(),
        command,
    }
}

fn comment(task: &str, text: &str) -> Command {
    Command::Task(TaskCommand::Comment {
        id: task.into(),
        text: text.into(),
        author: "owner".into(),
        send: false,
    })
}

fn comments(fixture: &Fixture, task: &str) -> usize {
    fixture.ctx.store.get(task).unwrap().comments.len()
}

fn task(fixture: &Fixture) -> String {
    fixture
        .ctx
        .store
        .create("target", None, Status::Ready, "")
        .unwrap()
        .id
}

#[test]
fn replayed_cmd_id_returns_stored_result_without_second_effect() {
    let fixture = Fixture::new();
    let id = task(&fixture);
    let mut connection = Connection::open(&fixture);
    let submit = request(&fixture, "cli-replay-1", comment(&id, "once"));
    let (first, first_output) = connection.submit(&submit).unwrap();
    assert_eq!(first.state, CmdState::Done);
    assert!(!first.replayed);
    let (second, second_output) = connection.submit(&submit).unwrap();
    assert_eq!(second.state, CmdState::Done);
    assert_eq!(second.exit, 0);
    assert!(second.replayed);
    assert!(first_output.is_some());
    assert_eq!(second_output, first_output);
    assert_eq!(comments(&fixture, &id), 1);
    connection.finish();
}

#[test]
fn stale_task_revision_expect_conflicts_without_effect() {
    let fixture = Fixture::new();
    let id = task(&fixture);
    let mut connection = Connection::open(&fixture);
    let mut submit = request(&fixture, "cli-expect-1", comment(&id, "guarded"));
    submit.expect = Some(Expect::TaskRevision {
        task: id.clone(),
        revision: "0".into(),
    });
    let (result, output) = connection.submit(&submit).unwrap();
    assert_eq!(result.state, CmdState::Conflict);
    assert_eq!(result.exit, 1);
    assert_eq!(result.error.unwrap().code, "conflict");
    assert_eq!(
        output.unwrap().stderr,
        format!("error: conflict: {id} changed since revision 0\n")
    );
    assert_eq!(comments(&fixture, &id), 0);
    connection.finish();
}

#[test]
fn reused_cmd_id_with_other_command_is_invalid_params() {
    let fixture = Fixture::new();
    let id = task(&fixture);
    let mut connection = Connection::open(&fixture);
    let (first, _) = connection
        .submit(&request(&fixture, "cli-reuse-1", comment(&id, "first")))
        .unwrap();
    assert_eq!(first.state, CmdState::Done);
    let error = connection
        .submit(&request(&fixture, "cli-reuse-1", comment(&id, "second")))
        .unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::InvalidParams);
    assert_eq!(error.message, "cmd_id_reused");
    assert_eq!(comments(&fixture, &id), 1);
    connection.finish();
}

#[test]
fn lost_result_query_tombstones_absent_command() {
    let fixture = Fixture::new();
    let id = task(&fixture);
    let mut connection = Connection::open(&fixture);
    let query = CmdResultQuery {
        cmd_id: "cli-lost-1".into(),
        wait_ms: 0,
        abandon_if_absent: true,
    };
    let (result, output) = connection.command(CMD_RESULT, &query).unwrap();
    assert_eq!(result.state, CmdState::Abandoned);
    assert_eq!(result.exit, 1);
    assert!(output.is_none());
    // The late submit the client lost track of must not run after its local fallback did.
    let error = connection
        .submit(&request(&fixture, "cli-lost-1", comment(&id, "late")))
        .unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::InvalidParams);
    assert_eq!(error.message, "cmd_id_reused");
    assert_eq!(comments(&fixture, &id), 0);
    connection.finish();
}

#[test]
fn pm_caller_attribution_survives_daemon_execution() {
    let fixture = Fixture::new();
    let id = task(&fixture);
    let mut connection = Connection::open(&fixture);
    let mut submit = request(&fixture, "cli-pm-1", comment(&id, "pm note"));
    submit.caller.actor = Some(ActorName::Pm);
    submit.caller.via = Via::Pm;
    submit.caller.env.corral_pm = Some("1".into());
    let (result, _) = connection.submit(&submit).unwrap();
    assert_eq!(result.state, CmdState::Done);
    let events = fixture.ctx.store.events_for(&id).unwrap();
    let commented = events
        .iter()
        .rev()
        .find(|event| event.kind == "commented")
        .unwrap();
    assert_eq!(commented.actor, Some(Actor::Pm));
    assert_eq!(commented.via.as_deref(), Some("pm"));
    connection.finish();
}

#[test]
fn daemon_outcome_bytes_equal_local_outcome_bytes() {
    let fixture = Fixture::new();
    let id = task(&fixture);
    let mut connection = Connection::open(&fixture);
    // One success and one executor failure: stdout, stderr, exit and error must not drift.
    for (n, text) in ["same bytes", "   "].into_iter().enumerate() {
        let submit = request(&fixture, &format!("cli-bytes-{n}"), comment(&id, text));
        let invoker = Invoker {
            caller: submit.caller.clone(),
            owner_action: None,
        };
        let (daemon, output) = connection.submit(&submit).unwrap();
        let output = output.unwrap();
        let local = corral_ops::command::local::run(&fixture.root, &invoker, comment(&id, text));
        assert_eq!(daemon.state, local.state);
        assert_eq!(daemon.exit, local.exit);
        assert_eq!(daemon.error, local.error);
        assert_eq!(output.stdout, local.stdout);
        assert_eq!(output.stderr, local.stderr);
    }
    assert_eq!(comments(&fixture, &id), 2);
    connection.finish();
}
