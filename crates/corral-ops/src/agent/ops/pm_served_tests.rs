//! Served agent PM read regressions.
use super::*;
use corral_protocol::{
    frame::{Frame, FrameKind, read_frame, write_frame},
    read::{ReadKind, ReadReply, ReadRequest},
    rpc::{self, ResponseOutcome},
};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

struct PrivateSocket(Option<PathBuf>);
impl Drop for PrivateSocket {
    fn drop(&mut self) {
        crate::daemon_client::SERVED_SOCKET.set(self.0.take());
    }
}

fn receive(stream: &mut UnixStream) -> rpc::Request {
    let frame = read_frame(stream).unwrap().unwrap();
    assert_eq!(frame.kind, FrameKind::Control);
    serde_json::from_slice(&frame.payload).unwrap()
}

fn respond(stream: &mut UnixStream, id: Value, outcome: ResponseOutcome<Value>) {
    let response = rpc::Response {
        jsonrpc: rpc::JSONRPC.into(),
        id,
        outcome,
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

fn daemon(fx: &ts::Fx, socket: &Path, refuse: bool) -> std::thread::JoinHandle<()> {
    let listener = UnixListener::bind(socket).unwrap();
    let ctx = Ctx {
        store: Store::open_unobserved(fx.ctx.store.root()).unwrap(),
        ..fx.ctx.clone()
    };
    std::thread::spawn(move || {
        for kind in [ReadKind::AgentPmBrief, ReadKind::AgentPmNext] {
            let ready = rustix::event::poll(
                &mut [rustix::event::PollFd::new(
                    &listener,
                    rustix::event::PollFlags::IN,
                )],
                Some(&rustix::event::Timespec {
                    tv_sec: 5,
                    tv_nsec: 0,
                }),
            )
            .unwrap();
            assert_eq!(ready, 1, "agent never requested the served read");
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let hello = receive(&mut stream);
            assert_eq!(hello.method, corral_protocol::HELLO);
            respond(
                &mut stream,
                hello.id,
                ResponseOutcome::Result {
                    result: json!({
                        "proto":{"major":corral_protocol::PROTO_MAJOR,"minor":corral_protocol::PROTO_MINOR},
                        "caps":[corral_protocol::READ_V1],
                        "daemon":{"version":"test","store":ctx.store.dir(),"epoch":1},
                        "health":corral_protocol::Health::down(),
                    }),
                },
            );
            let request = receive(&mut stream);
            assert_eq!(request.method, corral_protocol::READ);
            let params: ReadRequest = serde_json::from_value(request.params).unwrap();
            assert_eq!(params.kind, kind);
            assert_eq!(
                (params.exe, params.build),
                crate::pm_read::client_identity().unwrap()
            );
            if refuse {
                respond(
                    &mut stream,
                    request.id,
                    ResponseOutcome::Error {
                        error: rpc::Error {
                            code: rpc::ErrorCode::Refused,
                            message: "client_mismatch".into(),
                            data: None,
                        },
                    },
                );
                continue;
            }
            let seq = ctx.store.view().unwrap().seq().0;
            let value = match kind {
                ReadKind::AgentPmBrief => crate::pm_read::agent_brief(&ctx).unwrap(),
                ReadKind::AgentPmNext => crate::pm_read::agent_next(&ctx).unwrap(),
                _ => unreachable!(),
            };
            let bytes = serde_json::to_vec(&value).unwrap();
            respond(
                &mut stream,
                request.id,
                ResponseOutcome::Result {
                    result: serde_json::to_value(ReadReply {
                        as_of: Timestamp::now().to_string(),
                        seq,
                        cached: false,
                        stream: 1,
                        bytes: bytes.len() as u64,
                        frames: 1,
                    })
                    .unwrap(),
                },
            );
            write_frame(
                &mut stream,
                &Frame {
                    kind: FrameKind::Data,
                    stream: 1,
                    payload: bytes,
                },
            )
            .unwrap();
        }
    })
}

#[test]
fn served_and_fallback_pm_reads_have_identical_bytes_and_never_persist_read_receipts() {
    crate::pm_read::set_build_identity("test");
    let mut fx = ts::fx();
    fx.ctx.resource_probe = crate::resources::FixedResourceProbe::arc();
    fx.ctx.store.config.projects.insert(
        "corral".into(),
        serde_json::from_value(json!({"path":fx.ctx.store.root()})).unwrap(),
    );
    fx.ctx.store.save_config().unwrap();
    let task = task(&fx, "corral", "bounded read");
    let preview = fx
        .ctx
        .store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: [task.id].into(),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", Timestamp::now())
        .unwrap();
    let host = ts::bind(
        &fx,
        &BindTarget::Pm {
            project: agent::hooks::pm::WORKSPACE.into(),
        },
        "pm-pane",
        "pm-session",
    );
    ts::committed(&ts::hello(&fx, &host));
    let socket = fx._tmp.path().join("agent-reader.sock");
    let _private = PrivateSocket(crate::daemon_client::SERVED_SOCKET.replace(Some(socket.clone())));
    let before = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    let events = serde_json::to_vec(fx.ctx.store.events_shared().unwrap().as_ref()).unwrap();
    let mut local = Vec::new();
    // Absent daemon, served computation, then explicit refusal. All calls use
    // real admission and response construction, and private framed sockets.
    for path in 0..3 {
        let server = (path > 0).then(|| daemon(&fx, &socket, path == 2));
        for (index, op) in ["read_brief", "read_next"].into_iter().enumerate() {
            let response = send(&fx, &host, op, &format!("{path}-{op}"), json!({}));
            assert!(matches!(
                response,
                CallResponse::Receipt {
                    replayed: false,
                    ..
                }
            ));
            let bytes = serde_json::to_vec(&ts::committed(&response).result).unwrap();
            if path == 0 {
                local.push(bytes);
            } else {
                assert_eq!(bytes, local[index]);
            }
            assert_eq!(before, pm_state::load(&fx.ctx.store).unwrap().unwrap());
            assert_eq!(
                events,
                serde_json::to_vec(fx.ctx.store.events_shared().unwrap().as_ref()).unwrap()
            );
        }
        if let Some(server) = server {
            server.join().unwrap();
            std::fs::remove_file(&socket).unwrap();
        }
    }
}
