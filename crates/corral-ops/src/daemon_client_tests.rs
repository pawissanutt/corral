#![cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]

use std::os::unix::net::UnixListener;

use super::*;
use corral_protocol::DaemonInfo;
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
