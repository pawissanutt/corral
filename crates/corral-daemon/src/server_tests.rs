use super::*;
use crate::{Exit, RunOptions, run_with_pass, tests::Fixture};
use corral_core::{Status, Store};
use corral_ops::{boardview::Projection, daemon_client};
use corral_protocol::{
    board::BoardDelta,
    frame::{CONTROL_MAX, read_frame, write_frame},
    subscription::{SubAck, SubClose, SubDelta, SubOpen, Topic},
};
use std::sync::mpsc;

fn send_request(stream: &mut UnixStream, method: &str, params: impl Serialize) {
    let request = rpc::Request {
        jsonrpc: rpc::JSONRPC.into(),
        id: Value::from(1),
        method: method.into(),
        params,
    };
    write_frame(
        stream,
        &Frame {
            kind: FrameKind::Control,
            stream: 0,
            payload: serde_json::to_vec(&request).unwrap(),
        },
    )
    .unwrap();
}

fn result(stream: &mut UnixStream) -> Value {
    let frame = read_frame(stream).unwrap().unwrap();
    let response: rpc::Response<Value> = serde_json::from_slice(&frame.payload).unwrap();
    assert_eq!(response.id, Value::from(1));
    match response.outcome {
        rpc::ResponseOutcome::Result { result } => result,
        other => panic!("unexpected RPC error: {other:?}"),
    }
}

fn call(stream: &mut UnixStream, method: &str, params: impl Serialize) {
    send_request(stream, method, params);
    result(stream);
}

fn subscriber(store: &Store) -> UnixStream {
    let mut stream = UnixStream::connect(daemon_client::socket_path(store)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
    send_request(
        &mut stream,
        wire::HELLO,
        wire::Hello {
            proto: wire::Proto {
                major: wire::PROTO_MAJOR,
                minor: wire::PROTO_MINOR,
            },
            caps: vec![wire::BOARD_V1.into()],
            client: wire::ClientInfo {
                kind: "test".into(),
                version: "0".into(),
            },
        },
    );
    let welcome: wire::Welcome = serde_json::from_value(result(&mut stream)).unwrap();
    assert_eq!(welcome.caps, [wire::BOARD_V1]);
    call(
        &mut stream,
        wire::SUB_OPEN,
        SubOpen {
            topic: Topic::Board,
            since: None,
        },
    );
    stream
}

fn delta(stream: &mut UnixStream) -> BoardDelta {
    let frame = read_frame(stream).unwrap().unwrap();
    assert!(frame.payload.len() <= CONTROL_MAX);
    let message: rpc::Notification<SubDelta> = serde_json::from_slice(&frame.payload).unwrap();
    assert_eq!(message.method, wire::SUB_DELTA);
    let SubDelta::Board { delta } = message.params else {
        panic!("wrong topic")
    };
    delta
}

#[test]
fn subscription_pages_once_per_ack_and_nonacking_client_cannot_delay_a_pass() {
    let fixture = Fixture::new();
    let mut expected = Vec::new();
    for n in 0..101 {
        expected.push(
            fixture
                .ctx
                .store
                .create(&format!("task {n}"), None, Status::Ready, "")
                .unwrap()
                .id,
        );
    }
    let ctx = fixture.ctx.clone();
    let (started, ready) = mpsc::sync_channel(0);
    let (release, blocked) = mpsc::sync_channel(0);
    let daemon = std::thread::spawn(move || {
        let mut count = 0;
        run_with_pass(ctx, RunOptions { once: false }, |ctx, _| {
            count += 1;
            started.send(count).unwrap();
            if count == 1 {
                blocked.recv().unwrap();
            } else {
                daemon_client::Client::connect(&ctx.store)
                    .unwrap()
                    .unwrap()
                    .stop(false)
                    .unwrap();
            }
            None
        })
    });
    assert_eq!(ready.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
    let mut slow = subscriber(&fixture.ctx.store);
    let first = delta(&mut slow);
    assert!(first.more);
    let mut paged = subscriber(&fixture.ctx.store);
    let mut received = Vec::new();
    loop {
        let page = delta(&mut paged);
        received.extend(page.rows.into_iter().map(|row| row.id));
        assert!(page.left.is_empty());
        if !page.more {
            call(
                &mut paged,
                wire::SUB_CLOSE,
                SubClose {
                    topic: Topic::Board,
                },
            );
            break;
        }
        call(
            &mut paged,
            wire::SUB_ACK,
            SubAck {
                topic: Topic::Board,
                cursor: page.cursor,
            },
        );
    }
    received.sort();
    expected.sort();
    assert_eq!(received, expected); // Duplicates are not hidden by a map/set.
    assert_eq!(
        rustix::event::poll(
            &mut [rustix::event::PollFd::new(
                &slow,
                rustix::event::PollFlags::IN
            )],
            Some(&rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: 600_000_000
            }),
        )
        .unwrap(),
        0
    );
    let mut client = daemon_client::Client::connect(&fixture.ctx.store)
        .unwrap()
        .unwrap();
    client.nudge("test", None).unwrap();
    release.send(()).unwrap();
    assert_eq!(ready.recv_timeout(Duration::from_secs(5)).unwrap(), 2);
    assert_eq!(daemon.join().unwrap().unwrap(), Exit::Stopped);
}

#[test]
fn second_store_write_pushes_only_changed_row_without_another_pass() {
    let fixture = Fixture::new();
    let task = fixture
        .ctx
        .store
        .create("first", None, Status::Ready, "")
        .unwrap();
    fixture
        .ctx
        .store
        .create("untouched", None, Status::Ready, "")
        .unwrap();
    let ctx = fixture.ctx.clone();
    let (started, ready) = mpsc::sync_channel(0);
    let (release, blocked) = mpsc::sync_channel(0);
    let daemon = std::thread::spawn(move || {
        run_with_pass(ctx, RunOptions { once: false }, |_, _| {
            started.send(()).unwrap();
            blocked.recv().unwrap();
            None
        })
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut stream = subscriber(&fixture.ctx.store);
    let initial = delta(&mut stream);
    assert_eq!(initial.rows.len(), 2);
    call(
        &mut stream,
        wire::SUB_ACK,
        SubAck {
            topic: Topic::Board,
            cursor: initial.cursor,
        },
    );
    let other = Store::open(&fixture.root).unwrap();
    other
        .update(&task.id, |task| {
            task.title = "changed from another process".into();
            Ok(())
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let changed = loop {
        assert!(
            Instant::now() < deadline,
            "external write never reached the projection"
        );
        let page = delta(&mut stream);
        if !page.rows.is_empty() {
            break page;
        }
        call(
            &mut stream,
            wire::SUB_ACK,
            SubAck {
                topic: Topic::Board,
                cursor: page.cursor,
            },
        );
    };
    assert_eq!(changed.rows.len(), 1);
    assert_eq!(changed.rows[0].id, task.id);
    assert_eq!(changed.rows[0].title, "changed from another process");
    assert!(changed.left.is_empty());
    assert!(!changed.more);
    assert!(ready.try_recv().is_err());
    daemon_client::Client::connect(&fixture.ctx.store)
        .unwrap()
        .unwrap()
        .stop(false)
        .unwrap();
    release.send(()).unwrap();
    assert_eq!(daemon.join().unwrap().unwrap(), Exit::Stopped);
}

#[test]
fn oversized_board_pages_are_halved_below_control_frame_limit() {
    let fixture = Fixture::new();
    for _ in 0..100 {
        fixture
            .ctx
            .store
            .create(&"x".repeat(1_000), None, Status::Ready, "")
            .unwrap();
    }
    let shared = Shared::new();
    *shared.projection() = Some(Projection::rebuild(&fixture.ctx).unwrap());
    let mut topics = topics::Topics::new(&shared);
    topics
        .open(SubOpen {
            topic: Topic::Board,
            since: None,
        })
        .unwrap();
    let (mut server, mut client) = UnixStream::pair().unwrap();
    let mut received = Vec::new();
    client.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    let mut pages = 0;
    loop {
        topics.flush(&mut server).unwrap();
        let page = delta(&mut client);
        pages += 1;
        assert!(page.rows.len() < 100);
        received.extend(page.rows.into_iter().map(|row| row.id));
        if !page.more {
            break;
        }
        topics
            .ack(SubAck {
                topic: Topic::Board,
                cursor: page.cursor,
            })
            .unwrap();
    }
    received.sort();
    received.dedup();
    assert_eq!(received.len(), 100);
    assert!(pages > 1);
}

#[test]
fn draining_stop_reply_is_written_before_daemon_run_returns() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx.clone();
    let shared = Arc::new(Shared::new());
    let daemon_shared = Arc::clone(&shared);
    let (started, ready) = mpsc::sync_channel(0);
    let (release, blocked) = mpsc::sync_channel(0);
    let (reader, receive_reader) = mpsc::sync_channel::<UnixStream>(1);
    let daemon = std::thread::spawn(move || {
        let exit = crate::run_with_state(
            ctx,
            RunOptions { once: false },
            |_, _| {
                started.send(()).unwrap();
                blocked.recv().unwrap();
                None
            },
            Arc::clone(&daemon_shared),
        )
        .unwrap();
        assert_eq!(
            daemon_shared.lock().workers,
            0,
            "run returned before shutdown writers finished"
        );
        // Do not wait for the connection worker after run returns: the complete
        // Stopped frame must already be in the socket, as if the CLI exited here.
        let mut stream = receive_reader.recv().unwrap();
        stream.set_nonblocking(true).unwrap();
        let stopped: wire::Stopped = serde_json::from_value(result(&mut stream)).unwrap();
        (exit, stopped)
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    // A raw handshaken connection lets the test inspect the actual Stopped bytes.
    let mut stream = UnixStream::connect(daemon_client::socket_path(&fixture.ctx.store)).unwrap();
    stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    call(
        &mut stream,
        wire::HELLO,
        wire::Hello {
            proto: wire::Proto {
                major: wire::PROTO_MAJOR,
                minor: wire::PROTO_MINOR,
            },
            caps: Vec::new(),
            client: wire::ClientInfo {
                kind: "test".into(),
                version: "0".into(),
            },
        },
    );
    send_request(&mut stream, wire::DAEMON_STOP, wire::Stop { drain: true });
    reader.send(stream).unwrap();
    let (state, _) = shared
        .changed
        .wait_timeout_while(shared.lock(), Duration::from_secs(5), |state| !state.stop)
        .unwrap();
    assert!(state.stop, "daemon never accepted draining stop");
    drop(state);
    release.send(()).unwrap();
    assert_eq!(daemon.join().unwrap().0, Exit::Stopped);
}

#[test]
fn health_subscriber_that_acks_receives_no_delta_for_clock_ticks() {
    let shared = Shared::new();
    {
        let mut state = shared.lock();
        state.finished = Some(Instant::now() - Duration::from_secs(1));
        state.failures = 3;
        state.last_success = Instant::now() - Duration::from_secs(301);
        state.locks = vec!["lock dispatch held 95s by pid 1".into()];
    }
    let mut topics = topics::Topics::new(&shared);
    topics
        .open(SubOpen {
            topic: Topic::Health,
            since: None,
        })
        .unwrap();
    let (mut server, mut client) = UnixStream::pair().unwrap();
    client.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    topics.flush(&mut server).unwrap();
    let frame = read_frame(&mut client).unwrap().unwrap();
    let message: rpc::Notification<SubDelta> = serde_json::from_slice(&frame.payload).unwrap();
    assert_eq!(message.method, wire::SUB_DELTA);
    let SubDelta::Health { cursor, rows, .. } = message.params else {
        panic!("wrong topic");
    };
    assert_eq!(rows.len(), 1);
    assert!(rows[0].loop_facts.age_secs.is_some());
    topics
        .ack(SubAck {
            topic: Topic::Health,
            cursor,
        })
        .unwrap();
    for age in [2, 3] {
        let mut state = shared.lock();
        state.finished = Some(Instant::now() - Duration::from_secs(age));
        state.last_success = Instant::now() - Duration::from_secs(300 + age);
        state.locks = vec![format!("lock dispatch held {}s by pid 1", 94 + age)];
        drop(state);
        topics.flush(&mut server).unwrap();
    }
    assert_eq!(
        rustix::event::poll(
            &mut [rustix::event::PollFd::new(
                &client,
                rustix::event::PollFlags::IN
            )],
            Some(&rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: 0
            }),
        )
        .unwrap(),
        0
    );
}

fn read_connection(
    fixture: &Fixture,
    shared: &Arc<Shared>,
) -> (UnixStream, std::thread::JoinHandle<Result<()>>) {
    let socket = fixture.root.join("read-test.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let worker_shared = Arc::clone(shared);
    let store = fixture.ctx.store.dir().display().to_string();
    let worker = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        serve(stream, worker_shared, &store)
    });
    let mut stream = UnixStream::connect(&socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
    send_request(
        &mut stream,
        wire::HELLO,
        wire::Hello {
            proto: wire::Proto {
                major: wire::PROTO_MAJOR,
                minor: wire::PROTO_MINOR,
            },
            caps: vec![wire::READ_V1.into()],
            client: wire::ClientInfo {
                kind: "test".into(),
                version: "0".into(),
            },
        },
    );
    let welcome: wire::Welcome = serde_json::from_value(result(&mut stream)).unwrap();
    assert_eq!(welcome.caps, [wire::READ_V1]);
    (stream, worker)
}

fn read_request(kind: ReadKind) -> ReadRequest {
    corral_ops::pm_read::set_build_identity("test");
    let (exe, build) = corral_ops::pm_read::client_identity().unwrap();
    ReadRequest { kind, exe, build }
}

fn read_document(stream: &mut UnixStream) -> (ReadReply, Vec<u8>) {
    let reply: ReadReply = serde_json::from_value(result(stream)).unwrap();
    assert_ne!(reply.stream, 0);
    assert_eq!(
        reply.frames as usize,
        (reply.bytes as usize)
            .div_ceil(wire::frame::DATA_MAX)
            .max(1)
    );
    let mut bytes = Vec::new();
    for _ in 0..reply.frames {
        let frame = read_frame(stream).unwrap().unwrap();
        assert_eq!(frame.kind, FrameKind::Data);
        assert_eq!(frame.stream, reply.stream);
        assert!(frame.payload.len() <= wire::frame::DATA_MAX);
        bytes.extend(frame.payload);
    }
    assert_eq!(bytes.len() as u64, reply.bytes);
    (reply, bytes)
}

fn finish_read_connection(
    mut stream: UnixStream,
    shared: &Shared,
    worker: std::thread::JoinHandle<Result<()>>,
) {
    call(&mut stream, wire::BYE, ());
    worker.join().unwrap().unwrap();
    shared.close();
}

#[test]
fn multi_frame_read_is_complete_and_cannot_interleave_with_the_next_response() {
    let fixture = Fixture::new();
    let shared = Arc::new(Shared::new());
    let text = format!(
        "{}{}tail",
        "a".repeat(wire::frame::DATA_MAX),
        "b".repeat(wire::frame::DATA_MAX)
    );
    let expected = serde_json::to_vec(&serde_json::json!({"text": text})).unwrap();
    let document = expected.clone();
    crate::reader::start_with_compute(&fixture.ctx, &shared, move |_, kind, _| {
        Ok(if kind == ReadKind::PmNext {
            document.clone()
        } else {
            Vec::new()
        })
    })
    .unwrap();
    let (mut stream, worker) = read_connection(&fixture, &shared);
    send_request(&mut stream, wire::READ, read_request(ReadKind::PmNext));
    send_request(&mut stream, wire::PING, wire::Ping {});
    let (reply, bytes) = read_document(&mut stream);
    assert!(reply.frames > 1);
    assert_eq!(bytes, expected);
    assert!(!reply.cached);
    let _: wire::Pong = serde_json::from_value(result(&mut stream)).unwrap();
    send_request(&mut stream, wire::READ, read_request(ReadKind::PmBrief));
    let (reply, bytes) = read_document(&mut stream);
    assert_eq!(reply.frames, 1);
    assert!(bytes.is_empty());
    finish_read_connection(stream, &shared, worker);
}

#[test]
fn executable_mismatch_is_refused_before_computing_a_read() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let fixture = Fixture::new();
    let shared = Arc::new(Shared::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let computed = Arc::clone(&calls);
    crate::reader::start_with_compute(&fixture.ctx, &shared, move |_, _, _| {
        computed.fetch_add(1, Ordering::SeqCst);
        Ok(b"{}".to_vec())
    })
    .unwrap();
    let (mut stream, worker) = read_connection(&fixture, &shared);
    let mut request = read_request(ReadKind::PmNext);
    request.exe.push_str("-different");
    send_request(&mut stream, wire::READ, request);
    let frame = read_frame(&mut stream).unwrap().unwrap();
    let response: rpc::Response<Value> = serde_json::from_slice(&frame.payload).unwrap();
    let rpc::ResponseOutcome::Error { error } = response.outcome else {
        panic!("mismatched executable was served");
    };
    assert_eq!(error.code, rpc::ErrorCode::Refused);
    assert_eq!(error.message, "client_mismatch");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    finish_read_connection(stream, &shared, worker);
}

#[test]
fn repeated_read_computes_once_until_the_store_sequence_changes() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let fixture = Fixture::new();
    let shared = Arc::new(Shared::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let computed = Arc::clone(&calls);
    crate::reader::start_with_compute(&fixture.ctx, &shared, move |_, _, _| {
        computed.fetch_add(1, Ordering::SeqCst);
        Ok(b"{}".to_vec())
    })
    .unwrap();
    let (mut stream, worker) = read_connection(&fixture, &shared);
    send_request(&mut stream, wire::READ, read_request(ReadKind::PmNext));
    let (first, _) = read_document(&mut stream);
    send_request(&mut stream, wire::READ, read_request(ReadKind::PmNext));
    let (second, _) = read_document(&mut stream);
    assert!(!first.cached);
    assert!(second.cached);
    assert_eq!(first.seq, second.seq);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    fixture
        .ctx
        .store
        .create("Changed", None, Status::Ready, "")
        .unwrap();
    send_request(&mut stream, wire::READ, read_request(ReadKind::PmNext));
    let (third, _) = read_document(&mut stream);
    assert!(!third.cached);
    assert!(third.seq > second.seq);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    finish_read_connection(stream, &shared, worker);
}
