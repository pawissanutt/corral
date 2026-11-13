#![cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]

use std::os::unix::net::UnixListener;

use super::*;
use corral_protocol::DaemonInfo;
use corral_protocol::frame::{read_frame, write_frame};
use serde_json::json;

fn request(stream: &mut UnixStream) -> Request {
    let frame = read_frame(stream).unwrap().unwrap();
    assert_eq!(frame.kind, FrameKind::Control);
    assert_eq!(frame.stream, 0);
    serde_json::from_slice(&frame.payload).unwrap()
}

fn respond(stream: &mut UnixStream, request: Request, result: impl Serialize) {
    let response = Response {
        jsonrpc: JSONRPC.into(),
        id: request.id,
        outcome: ResponseOutcome::Result { result },
    };
    write_frame(
        stream,
        &Frame {
            kind: FrameKind::Control,
            stream: 0,
            payload: serde_json::to_vec(&response).unwrap(),
        },
    )
    .unwrap();
}

#[test]
fn waited_nudge_preserves_ack_and_rejects_incompatible_welcome() {
    let dir = tempfile::tempdir().unwrap();
    for major in [PROTO_MAJOR, 2] {
        let socket = dir.path().join(format!("daemon-{major}.sock"));
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
            stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
            let hello = request(&mut stream);
            assert_eq!(hello.method, corral_protocol::HELLO);
            let params: Hello = serde_json::from_value(hello.params.clone()).unwrap();
            assert_eq!(params.proto.major, PROTO_MAJOR);
            respond(
                &mut stream,
                hello,
                Welcome {
                    proto: Proto { major, minor: 0 },
                    caps: Vec::new(),
                    daemon: DaemonInfo {
                        version: "test".into(),
                        store: "fixture".into(),
                        epoch: 1,
                    },
                    health: Health::down(),
                },
            );
            if major == PROTO_MAJOR {
                let nudge = request(&mut stream);
                assert_eq!(nudge.method, corral_protocol::NUDGE);
                assert_eq!(nudge.params, json!({"source":"fixture","wait_ms":750}));
                respond(
                    &mut stream,
                    nudge,
                    NudgeAck {
                        pass_seq: 42,
                        completed: true,
                    },
                );
            }
        });
        let client = Client::connect_at(&socket);
        if major == PROTO_MAJOR {
            let ack = client
                .unwrap()
                .unwrap()
                .nudge("fixture", Some(Duration::from_millis(750)))
                .unwrap();
            assert_eq!(
                ack,
                NudgeAck {
                    pass_seq: 42,
                    completed: true
                }
            );
        } else {
            assert!(client.is_err());
        }
        server.join().unwrap();
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn full_accept_backlog_cannot_block_best_effort_connect() {
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("full.sock");
    let listener = rustix::net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    )
    .unwrap();
    rustix::net::bind(&listener, &SocketAddrUnix::new(&path).unwrap()).unwrap();
    rustix::net::listen(&listener, 1).unwrap();
    let mut pending = Vec::new();
    // Every successful nonblocking connect occupies a backlog slot; stop only
    // when the kernel itself reports saturation, never after a guessed count.
    loop {
        let fd = rustix::net::socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        match rustix::net::connect(&fd, &SocketAddrUnix::new(&path).unwrap()) {
            Ok(()) => pending.push(fd),
            Err(rustix::io::Errno::AGAIN) => break,
            Err(error) => panic!("unexpected backlog fill error: {error}"),
        }
    }
    let (finished, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        finished.send(Client::connect_at(&path).is_err()).unwrap();
    });
    assert!(
        receive
            .recv_timeout(IO_TIMEOUT + Duration::from_millis(500))
            .unwrap()
    );
    worker.join().unwrap();
    drop(pending);
}

fn welcome_reader(stream: &mut UnixStream, read_capable: bool) {
    stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
    let hello = request(stream);
    assert_eq!(hello.method, corral_protocol::HELLO);
    let params: Hello = serde_json::from_value(hello.params.clone()).unwrap();
    assert!(
        params
            .caps
            .iter()
            .any(|cap| cap == corral_protocol::BOARD_V1)
    );
    assert!(
        params
            .caps
            .iter()
            .any(|cap| cap == corral_protocol::read::READ_V1)
    );
    let mut caps = vec![corral_protocol::BOARD_V1.into()];
    if read_capable {
        caps.push(corral_protocol::read::READ_V1.into());
    }
    respond(
        stream,
        hello,
        Welcome {
            proto: Proto {
                major: PROTO_MAJOR,
                minor: PROTO_MINOR,
            },
            caps,
            daemon: DaemonInfo {
                version: "test".into(),
                store: "fixture".into(),
                epoch: 1,
            },
            health: Health::down(),
        },
    );
}

#[test]
fn served_document_reassembles_multiple_data_frames() {
    crate::pm_read::set_build_identity("test");
    let identity = crate::pm_read::client_identity().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("reader.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let document = br#"{"json":{"kind":"wait"},"text":"workspace: waiting\n"}"#;
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        welcome_reader(&mut stream, true);
        let read = request(&mut stream);
        assert_eq!(read.method, corral_protocol::read::READ);
        let params: ReadRequest = serde_json::from_value(read.params.clone()).unwrap();
        assert_eq!(params.kind, ReadKind::PmNext);
        assert_eq!((params.exe, params.build), identity);
        let chunks = document.chunks(11);
        respond(
            &mut stream,
            read,
            ReadReply {
                as_of: "2026-11-12T00:00:00Z".into(),
                seq: 7,
                cached: false,
                stream: 1,
                bytes: document.len() as u64,
                frames: chunks.len() as u32,
            },
        );
        for chunk in chunks {
            write_frame(
                &mut stream,
                &Frame {
                    kind: FrameKind::Data,
                    stream: 1,
                    payload: chunk.to_vec(),
                },
            )
            .unwrap();
        }
    });
    let mut client = Client::connect_at(&socket).unwrap().unwrap();
    assert_eq!(client.read(ReadKind::PmNext).unwrap().unwrap(), document);
    server.join().unwrap();
}

#[test]
fn served_document_refusal_is_a_local_fallback() {
    crate::pm_read::set_build_identity("test");
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("reader.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        welcome_reader(&mut stream, true);
        let read = request(&mut stream);
        assert_eq!(read.method, corral_protocol::read::READ);
        let response: Response = Response {
            jsonrpc: JSONRPC.into(),
            id: read.id,
            outcome: ResponseOutcome::Error {
                error: corral_protocol::rpc::Error {
                    code: corral_protocol::rpc::ErrorCode::Refused,
                    message: "client_mismatch".into(),
                    data: None,
                },
            },
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
    });
    let mut client = Client::connect_at(&socket).unwrap().unwrap();
    assert!(client.read(ReadKind::PmBrief).unwrap().is_none());
    server.join().unwrap();
}

#[test]
fn missing_read_capability_never_sends_a_read_request() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("reader.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        welcome_reader(&mut stream, false);
        // The next RPC must be the caller's nudge, not an unsupported read.
        let nudge = request(&mut stream);
        assert_eq!(nudge.method, corral_protocol::NUDGE);
        respond(
            &mut stream,
            nudge,
            NudgeAck {
                pass_seq: 1,
                completed: true,
            },
        );
    });
    let mut client = Client::connect_at(&socket).unwrap().unwrap();
    assert!(client.read(ReadKind::BoardReadback).unwrap().is_none());
    assert!(client.nudge("fixture", None).unwrap().completed);
    server.join().unwrap();
}
