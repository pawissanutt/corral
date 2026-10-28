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

fn churn(f: &Fixture, ids: &[String]) -> usize {
    f.ctx
        .store
        .events()
        .unwrap()
        .iter()
        .filter(|e| matches!(e.kind.as_str(), "queued" | "dispatch_failed"))
        .filter(|e| e.task.as_ref().is_some_and(|t| ids.contains(t)))
        .count()
}

#[test]
fn unrunnable_ready_tasks_stay_quiet_across_syncs_and_leave_review_launches_free() {
    // Given a task awaiting its gate and N authorized Ready tasks whose dependency is not done.
    let mut f = super::queue_fixture();
    f.ctx.store.config.review.gate = Some(vec!["true".into()]);
    f.ctx.store.save_config().unwrap();
    let reviewed = f.task("app", Status::Ready);
    f.ok(&["dispatch", &reviewed.id]);
    f.ok(&[
        "done",
        &reviewed.id,
        "--summary",
        "Verified: fixture\nNot verified: none",
    ]);
    let predecessor = f.task("other", Status::Backlog);
    let blocked: Vec<String> = (0..4)
        .map(|_| {
            let task = f.task("other", Status::Ready);
            f.ctx
                .store
                .update(&task.id, |t| {
                    t.depends_on.push(predecessor.id.clone());
                    Ok(())
                })
                .unwrap();
            corral_ops::enqueue(&f.ctx, &task.id, &DispatchOpts::default()).unwrap();
            task.id
        })
        .collect();
    let enqueued = churn(&f, &blocked);

    // When several sync passes run, some while another process holds the reconciliation lease.
    let lease = f
        .ctx
        .store
        .try_lock_shared("session-ownership")
        .unwrap()
        .unwrap();
    for _ in 0..3 {
        corral_ops::sync(&f.ctx).unwrap();
    }
    drop(lease);
    for _ in 0..3 {
        corral_ops::sync(&f.ctx).unwrap();
    }

    // Then no pass re-queues or re-fails the unchanged tasks, and the gate launch is admitted.
    assert_eq!(churn(&f, &blocked), enqueued);
    corral_ops::gate_start(&f.ctx, &reviewed.id).unwrap();
    for id in &blocked {
        assert!(f.ctx.store.get(id).unwrap().runs.is_empty());
    }

    // And the meaningful state change, the predecessor landing, admits the queued work again.
    f.ctx
        .store
        .update(&predecessor.id, |t| {
            t.status = Status::Done;
            Ok(())
        })
        .unwrap();
    corral_ops::sync(&f.ctx).unwrap();
    assert!(
        blocked
            .iter()
            .any(|id| f.ctx.store.get(id).unwrap().status == Status::Running)
    );
}

#[cfg(target_os = "linux")]
fn wrapped(f: &Fixture, prefix: &[&str], args: &[&str]) -> std::process::Command {
    let inner = f.command(args);
    let mut command = std::process::Command::new(prefix[0]);
    command
        .args(&prefix[1..])
        .arg(inner.get_program())
        .args(inner.get_args())
        .stdin(std::process::Stdio::null());
    for (key, value) in inner.get_envs() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    command
}

#[cfg(target_os = "linux")]
fn reviewed(f: &Fixture, project: &str) -> String {
    let task = f.task(project, Status::Ready);
    f.ok(&["dispatch", &task.id]);
    f.ok(&[
        "done",
        &task.id,
        "--summary",
        "Verified: fixture\nNot verified: none",
    ]);
    task.id
}

#[cfg(target_os = "linux")]
#[test]
fn reconciliation_waits_for_artifact_storage_before_taking_the_launch_lease() {
    use std::io::Read;
    use std::process::Stdio;
    // Given a finished gate that sync must collect and attach, and an unrelated task ready to gate.
    let mut f = Fixture::new();
    f.ctx.store.config.review.gate = Some(vec!["true".into()]);
    f.ctx.store.config.limits.running = 3;
    f.ctx.store.config.limits.review = 2;
    f.ctx.store.save_config().unwrap();
    let finished = reviewed(&f, "app");
    let unrelated = reviewed(&f, "other");
    f.ok(&["gate", &finished]);
    let script = corral_ops::gate_paths(&f.ctx, &finished, "R1").script;
    let ran = std::process::Command::new("bash")
        .arg(script)
        .env("CORRAL_ROOT", f.temp.path())
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(ran.status.success(), "{ran:?}");

    // When another process holds artifact storage while a real sync process starts.
    let lock = f.ctx.store.dir().join("lock/artifact-storage.lock");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let held = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock)
        .unwrap();
    held.lock().unwrap();
    let mut sync = wrapped(&f, &["strace", "-yy", "-e", "trace=flock"], &["sync"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut trace = sync.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut chunk = [0; 4096];
        while let Ok(n) = trace.read(&mut chunk) {
            if n == 0 || tx.send(chunk[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    // Await the exact blocking syscall entry, bounded, without polling.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen = Vec::new();
    while !String::from_utf8_lossy(&seen).contains("/artifact-storage.lock>, LOCK_EX") {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_else(|| {
                panic!(
                    "sync never reached storage: {}",
                    String::from_utf8_lossy(&seen)
                )
            });
        seen.extend(rx.recv_timeout(remaining).unwrap());
    }

    // Then an unrelated gate launch proceeds while that pass still waits for storage.
    let launch = wrapped(&f, &["timeout", "60"], &["gate", &unrelated])
        .output()
        .unwrap();
    assert!(launch.status.success(), "{launch:?}");
    assert!(sync.try_wait().unwrap().is_none(), "sync finished early");
    let started = f.ctx.store.get(&unrelated).unwrap();
    assert_eq!(
        started.current_run().unwrap().gate.as_ref().unwrap().result,
        corral_core::GateResult::Running
    );

    // And the pass collects the finished gate once storage is released.
    held.unlock().unwrap();
    assert!(sync.wait().unwrap().success());
    drop(rx);
    reader.join().unwrap();
    let collected = f.ctx.store.get(&finished).unwrap();
    assert_eq!(
        collected
            .current_run()
            .unwrap()
            .gate
            .as_ref()
            .unwrap()
            .result,
        corral_core::GateResult::Pass
    );
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
