use super::*;

#[test]
fn daemon_owned_tick_never_enters_in_process_sync() {
    let dir = tempfile::tempdir().unwrap();
    let owner = corral_core::Store::init(dir.path()).unwrap();
    assert!(owner.own_loop().unwrap());
    let store = corral_core::Store::open(dir.path()).unwrap();
    let ctx = Ctx {
        store,
        herdr: corral_herdr::Herdr::with_bin(dir.path().join("must-not-run"), None),
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
    };
    // An in-process sync would check runtime and latch off on this version.
    std::fs::write(ctx.store.dir().join("state/schema-version"), "999\n").unwrap();
    let (sender, _messages) = mpsc::channel();
    let _ = WorkerMsg::pass(&ctx, &sender, Path::new("/must-not-run"));
    assert!(!ctx.store.runtime_latched());
}

#[cfg(unix)]
#[test]
fn board_falls_back_to_local_frame_on_identity_mismatch() {
    use corral_protocol::frame::{Frame, FrameKind, read_frame, write_frame};
    use corral_protocol::rpc::{self, JSONRPC, Request, Response, ResponseOutcome};
    use ratatui::{Terminal, backend::TestBackend};
    use std::os::unix::net::{UnixListener, UnixStream};

    /// The next request, or None at the end of this connection.
    fn call(stream: &mut UnixStream) -> Option<Request> {
        let frame = read_frame(stream).unwrap()?;
        Some(serde_json::from_slice(&frame.payload).unwrap())
    }
    fn answer(stream: &mut UnixStream, id: serde_json::Value, outcome: ResponseOutcome) {
        let response = Response {
            jsonrpc: JSONRPC.into(),
            id,
            outcome,
        };
        let payload = serde_json::to_vec(&response).unwrap();
        let frame = Frame {
            kind: FrameKind::Control,
            stream: 0,
            payload,
        };
        write_frame(stream, &frame).unwrap();
    }
    /// Answers every request kind a board client can send; refuses every served read as an
    /// identity mismatch. Returns `(method, read kind)` per request, or None for the empty
    /// connection that ends the fake.
    fn serve(stream: &mut UnixStream) -> Option<Vec<(String, serde_json::Value)>> {
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut seen = Vec::new();
        while let Some(request) = call(stream) {
            let outcome = match request.method.as_str() {
                corral_protocol::HELLO => {
                    let welcome = corral_protocol::Welcome {
                        proto: corral_protocol::Proto {
                            major: corral_protocol::PROTO_MAJOR,
                            minor: 0,
                        },
                        caps: vec![
                            corral_protocol::READ_V1.into(),
                            corral_protocol::CLASSIC_V1.into(),
                        ],
                        daemon: corral_protocol::DaemonInfo {
                            version: "test".into(),
                            store: "fixture".into(),
                            epoch: 1,
                        },
                        health: corral_protocol::Health::down(),
                    };
                    let result = serde_json::to_value(welcome).unwrap();
                    ResponseOutcome::Result { result }
                }
                corral_protocol::READ => ResponseOutcome::Error {
                    error: rpc::Error {
                        code: rpc::ErrorCode::Refused,
                        message: "client identity does not match the daemon".into(),
                        data: None,
                    },
                },
                corral_protocol::NUDGE => {
                    let ack = corral_protocol::NudgeAck {
                        pass_seq: 0,
                        completed: true,
                    };
                    let result = serde_json::to_value(ack).unwrap();
                    ResponseOutcome::Result { result }
                }
                corral_protocol::SUB_OPEN
                | corral_protocol::SUB_ACK
                | corral_protocol::SUB_CLOSE => ResponseOutcome::Result {
                    result: serde_json::Value::Null,
                },
                _ => ResponseOutcome::Error {
                    error: rpc::Error {
                        code: rpc::ErrorCode::MethodNotFound,
                        message: "unknown method".into(),
                        data: None,
                    },
                },
            };
            seen.push((request.method.clone(), request.params["kind"].clone()));
            answer(stream, request.id, outcome);
        }
        (!seen.is_empty()).then_some(seen)
    }

    // Given a displayed board, then a store change it has not shown yet.
    let fx = crate::board::tests::seeded();
    let mut app = fx.app(crate::Mode::Board);
    fx.ctx
        .store
        .update("T-002", |task| {
            task.title = "Local frame".into();
            Ok(())
        })
        .unwrap();
    // And a daemon that refuses this client's identity for every served read. A served read
    // sends the client's build identity, which only the executable sets at startup.
    corral_ops::pm_read::set_build_identity("worker-test-build");
    let socket = corral_ops::daemon_client::socket_path(&fx.ctx.store);
    let runtime = socket.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&runtime).unwrap();
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = std::thread::spawn(move || {
        let mut seen = Vec::new();
        loop {
            let (mut stream, _) = listener.accept().unwrap();
            match serve(&mut stream) {
                Some(requests) => seen.extend(requests),
                None => return seen,
            }
        }
    });

    // When a daemon-owned worker update reads the frame.
    let mut classic = ClassicCache::new();
    let update = WorkerMsg::daemon(&fx.ctx, &mut classic, None);
    // Every client connection of the update has closed; an empty one ends the fake.
    drop(UnixStream::connect(&socket).unwrap());
    let seen = daemon.join().unwrap();
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir(&runtime);

    // Then the one request after hello was the refused classic frame read, and the frame came
    // from the local cache.
    let null = serde_json::Value::Null;
    assert_eq!(
        seen,
        [
            (corral_protocol::HELLO.to_owned(), null),
            (
                corral_protocol::READ.to_owned(),
                serde_json::json!("classic_frame")
            ),
        ]
    );
    assert!(classic.current().is_some());
    assert!(matches!(&update, WorkerMsg::Synced(Ok(_))));
    app.on_worker(update);
    let mut terminal = Terminal::new(TestBackend::new(160, 45)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let text = crate::board::tests::buffer_text(terminal.backend().buffer());
    assert!(text.contains("Local frame"), "{text}");
}
