use std::io;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::path::PathBuf;
use std::sync::Barrier;
use std::time::{Duration, Instant};

use corral_core::Status;
use corral_ops::{DispatchOpts, DispatchOutcome};
use serde_json::json;

use crate::queue_support::Fixture;

struct ProtocolBarrier {
    socket: UnixDatagram,
    point: String,
}

impl ProtocolBarrier {
    fn new(f: &Fixture, point: &str) -> Self {
        let socket = UnixDatagram::bind(f.temp.path().join("barrier-test.sock")).unwrap();
        std::fs::write(f.temp.path().join("barrier"), point).unwrap();
        Self {
            socket,
            point: point.into(),
        }
    }

    fn reached(&self) -> PathBuf {
        self.reached_with(|bytes| self.socket.recv_from(bytes))
    }

    fn reached_with(
        &self,
        mut receive: impl FnMut(&mut [u8]) -> io::Result<(usize, SocketAddr)>,
    ) -> PathBuf {
        let mut bytes = [0; 128];
        let deadline = Instant::now() + Duration::from_secs(10);
        let (len, peer) = loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("protocol barrier timed out");
            self.socket.set_read_timeout(Some(remaining)).unwrap();
            match receive(&mut bytes) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => break result.unwrap(),
            }
        };
        assert_eq!(&bytes[..len], self.point.as_bytes());
        peer.as_pathname().unwrap().to_path_buf()
    }

    fn release(&self, peer: &std::path::Path) {
        self.socket.send_to(b"1", peer).unwrap();
    }
}

#[test]
fn protocol_barrier_retries_interrupted_receive() {
    // Given a real, queued protocol datagram and an interrupted first receive.
    let f = super::queue_fixture();
    let barrier = ProtocolBarrier::new(&f, "claimed workspace list");
    let peer_path = f.temp.path().join("barrier-peer.sock");
    let peer = UnixDatagram::bind(&peer_path).unwrap();
    peer.send_to(
        barrier.point.as_bytes(),
        f.temp.path().join("barrier-test.sock"),
    )
    .unwrap();
    let mut interrupted = true;
    // When the barrier receives the transient syscall interruption.
    let reached = barrier.reached_with(|bytes| {
        if std::mem::take(&mut interrupted) {
            Err(io::ErrorKind::Interrupted.into())
        } else {
            barrier.socket.recv_from(bytes)
        }
    });
    // Then it still consumes the real protocol event and returns its peer.
    assert_eq!(reached, peer_path);
}

#[test]
fn competing_sync_and_manual_dispatch_launch_exactly_one_with_one_slot() {
    // Given one project slot, review headroom and a durably reserved manual launch.
    let mut f = super::queue_fixture();
    f.ctx
        .store
        .config
        .limits
        .per_project
        .insert("app".into(), 1);
    f.ctx.store.save_config().unwrap();
    let queued = f.task("app", Status::Ready);
    let manual = f.task("app", Status::Ready);
    corral_ops::enqueue(&f.ctx, &queued.id, &DispatchOpts::default()).unwrap();
    let barrier = ProtocolBarrier::new(&f, "claimed workspace list");
    let start = Barrier::new(2);
    std::thread::scope(|s| {
        let ctx = &f.ctx;
        let id = &manual.id;
        let dispatch = s.spawn(move || {
            corral_ops::dispatch(
                ctx,
                id,
                &DispatchOpts {
                    queue_if_blocked: true,
                    ..DispatchOpts::default()
                },
            )
        });
        let peer = barrier.reached();
        let reserved = f.ctx.store.get(&manual.id).unwrap();
        assert!(reserved.launch.as_ref().unwrap().reserved);
        assert_eq!(f.state()["creates"], 0);
        assert_eq!(f.state()["submits"], 0);
        // When sync overlaps the already-admitted worker's external launch.
        let sync = s.spawn(|| {
            start.wait();
            corral_ops::sync(ctx)
        });
        start.wait();
        barrier.release(&peer);
        sync.join().unwrap().unwrap();
        assert!(matches!(
            dispatch.join().unwrap().unwrap(),
            DispatchOutcome::Started { .. }
        ));
    });
    // Then the reservation prevents a second queued launch.
    assert_eq!(f.ctx.store.get(&manual.id).unwrap().runs.len(), 1);
    assert!(f.ctx.store.get(&queued.id).unwrap().runs.is_empty());
    assert_eq!(f.state()["creates"], 1);
    assert_eq!(f.state()["submits"], 1);
    assert_eq!(corral_ops::slots(&f.ctx).unwrap(), (1, 2));
}

#[test]
fn inventory_first_refuses_manual_dispatch_then_sync_launches_exactly_one() {
    // Given real sync cleanup paused while it retains the sampled inventory lease.
    let mut f = super::queue_fixture();
    f.ctx
        .store
        .config
        .limits
        .per_project
        .insert("app".into(), 1);
    f.ctx.store.save_config().unwrap();
    let queued = f.task("app", Status::Ready);
    let manual = f.task("app", Status::Ready);
    corral_ops::enqueue(&f.ctx, &queued.id, &DispatchOpts::default()).unwrap();
    let retired = f.task("app", Status::Done);
    f.ctx
        .store
        .update(&retired.id, |task| {
            task.runs.push(serde_json::from_value(json!({
                "id":"R1", "agent":"generic", "worktree":f.temp.path().join("retired"),
                "branch":"corral/retired", "base":"main", "base_sha":"abc",
                "started":"2026-10-15T00:00:00Z", "ended":"2026-10-15T01:00:00Z",
                "outcome":"done", "herdr":{
                    "workspace_id":"w0", "pane_id":"w0:p1",
                    "socket":f.ctx.herdr.socket_path()?
                }
            }))?);
            Ok(())
        })
        .unwrap();
    let barrier = ProtocolBarrier::new(&f, "before pane close");
    std::thread::scope(|s| {
        let sync = s.spawn(|| corral_ops::sync(&f.ctx));
        let peer = barrier.reached();
        assert!(f.ctx.store.try_lock("session-ownership").unwrap().is_none());
        let before = f.ctx.store.get(&manual.id).unwrap();
        let events = serde_json::to_value(f.ctx.store.events().unwrap()).unwrap();
        // When manual admission competes with that exact inventory consumption.
        let error = corral_ops::dispatch(
            &f.ctx,
            &manual.id,
            &DispatchOpts {
                queue_if_blocked: true,
                ..DispatchOpts::default()
            },
        )
        .unwrap_err();
        // Then T-094 refuses without publishing a run, queue claim or launch event.
        assert_eq!(
            error.to_string(),
            "pane inventory is being reconciled; retry dispatch"
        );
        assert_eq!(f.ctx.store.get(&manual.id).unwrap(), before);
        assert_eq!(
            serde_json::to_value(f.ctx.store.events().unwrap()).unwrap(),
            events
        );
        assert_eq!(f.state()["creates"], 0);
        assert_eq!(f.state()["submits"], 0);
        assert_eq!(corral_ops::slots(&f.ctx).unwrap(), (0, 2));
        barrier.release(&peer);
        sync.join().unwrap().unwrap();
    });
    assert!(f.ctx.store.get(&manual.id).unwrap().runs.is_empty());
    assert_eq!(f.ctx.store.get(&queued.id).unwrap().runs.len(), 1);
    assert_eq!(f.state()["creates"], 1);
    assert_eq!(f.state()["submits"], 1);
    assert_eq!(corral_ops::slots(&f.ctx).unwrap(), (1, 2));
}
