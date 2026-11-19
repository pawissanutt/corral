#![cfg(unix)]

use std::{fs, os::unix::net::UnixListener, path::Path, process::Command, thread, time::Duration};

use corral_engine::{Engine, NewTask, Op, Role, Run, SessionRef, Status};
use corral_protocol::{
    self as wire,
    frame::{Frame, FrameKind, read_frame, write_frame},
    rpc,
};
use serde_json::{Value, json};

#[test]
fn hook_nudges_the_daemon_and_syncs_when_the_socket_is_absent() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap();
    let dir = tempfile::Builder::new()
        .prefix(".h-")
        .tempdir_in(workspace)
        .unwrap();
    let engine = Engine::init(dir.path(), None).unwrap();
    fs::write(
        engine.state_dir.join("corral.toml"),
        "[runner]\nmode = 'process'\n",
    )
    .unwrap();
    let task = engine
        .apply(
            Op::New {
                task: NewTask {
                    title: "ended outside the engine".into(),
                    ..NewTask::default()
                },
            },
            false,
        )
        .unwrap()
        .task
        .unwrap();
    engine
        .apply(
            Op::Move {
                id: task.id.clone(),
                status: Status::Running,
            },
            true,
        )
        .unwrap();
    let run_dir = engine.state_dir.join("runs").join(&task.id).join("R1");
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(run_dir.join("exit_code"), "0\n").unwrap();
    engine
        .store
        .insert_run(&Run {
            id: "R1".into(),
            task: task.id.clone(),
            role: Role::Worker,
            profile: "generic".into(),
            harness: "generic".into(),
            model: None,
            effort: None,
            worktree: dir.path().into(),
            branch: "task".into(),
            base: "main".into(),
            base_sha: "base".into(),
            session: Some(SessionRef::Process {
                pid: 0,
                dir: run_dir,
            }),
            started: jiff::Timestamp::now(),
            ended: None,
            outcome: None,
            summary: None,
            head_sha: None,
            quiet_since: None,
        })
        .unwrap();

    let socket = engine.state_dir.join("daemon.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        for (method, result) in [
            (wire::HELLO, json!({})),
            (
                wire::NUDGE,
                json!(wire::NudgeAck {
                    pass_seq: 0,
                    completed: false
                }),
            ),
        ] {
            let frame = read_frame(&mut stream).unwrap().unwrap();
            let request: rpc::Request = serde_json::from_slice(&frame.payload).unwrap();
            assert_eq!(request.method, method);
            if method == wire::NUDGE {
                let nudge: wire::Nudge = serde_json::from_value(request.params).unwrap();
                assert_eq!(nudge.source, "hook");
            }
            let response: rpc::Response<Value> = rpc::Response {
                jsonrpc: rpc::JSONRPC.into(),
                id: request.id,
                outcome: rpc::ResponseOutcome::Result { result },
            };
            write_frame(
                &mut stream,
                &Frame {
                    kind: FrameKind::Control,
                    stream: 0,
                    payload: serde_json::to_vec(&response).unwrap(),
                },
            )
            .unwrap();
        }
    });
    let hook = || {
        let output = Command::new(env!("CARGO_BIN_EXE_corral-next"))
            .arg("--root")
            .arg(dir.path())
            .arg("hook")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    hook();
    server.join().unwrap();
    assert_eq!(
        engine.store.require_task(&task.id).unwrap().status,
        Status::Running
    );

    fs::remove_file(socket).unwrap();
    hook();
    assert_eq!(
        engine.store.require_task(&task.id).unwrap().status,
        Status::Blocked
    );
}
