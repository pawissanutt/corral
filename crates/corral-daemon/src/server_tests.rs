use super::*;
use crate::{Exit, RunOptions, run_with_pass, tests::Fixture};
use corral_ops::daemon_client;
use std::sync::mpsc;

#[test]
fn client_that_never_reads_cannot_delay_the_follow_up_pass() {
    let fixture = Fixture::new();
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
    let mut slow = UnixStream::connect(daemon_client::socket_path(&fixture.ctx.store)).unwrap();
    let hello = rpc::Request {
        jsonrpc: rpc::JSONRPC.into(),
        id: Value::from(1),
        method: wire::HELLO.into(),
        params: wire::Hello {
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
    };
    write_frame(
        &mut slow,
        &Frame {
            kind: FrameKind::Control,
            stream: 0,
            payload: serde_json::to_vec(&hello).unwrap(),
        },
    )
    .unwrap();
    let mut client = daemon_client::Client::connect(&fixture.ctx.store)
        .unwrap()
        .unwrap();
    assert!(!client.nudge("test", None).unwrap().completed);
    release.send(()).unwrap();
    assert_eq!(ready.recv_timeout(Duration::from_secs(5)).unwrap(), 2);
    assert_eq!(daemon.join().unwrap().unwrap(), Exit::Stopped);
    drop(slow);
}
