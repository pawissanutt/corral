use std::io;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::path::PathBuf;
use std::sync::Barrier;
use std::time::{Duration, Instant};

use corral_core::Status;
use corral_ops::{DispatchOpts, DispatchOutcome};
use serde_json::json;

use crate::queue_support::Fixture;

#[cfg(target_os = "linux")]
#[path = "settlement.rs"]
mod settlement;

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
struct Traced {
    child: std::process::Child,
    trace: std::sync::mpsc::Receiver<Vec<u8>>,
    seen: Vec<u8>,
    reader: std::thread::JoinHandle<()>,
}

#[cfg(target_os = "linux")]
impl Traced {
    fn sync(f: &Fixture, strace: &[&str]) -> Self {
        Self::spawn(f, strace, &["sync"])
    }

    /// A traced corral command in its own process group, so a failing test can kill it.
    fn spawn(f: &Fixture, strace: &[&str], args: &[&str]) -> Self {
        use std::io::Read;
        use std::os::unix::process::CommandExt;
        let mut child = wrapped(f, strace, args)
            .process_group(0)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (tx, trace) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut chunk = [0; 4096];
            while let Ok(n) = stderr.read(&mut chunk) {
                if n == 0 || tx.send(chunk[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            trace,
            seen: Vec::new(),
            reader,
        }
    }

    /// Awaits traced syscall text with a bounded deadline, without polling.
    fn until(&mut self, reached: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !reached(&String::from_utf8_lossy(&self.seen)) {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_else(|| {
                    panic!(
                        "trace never matched: {}",
                        String::from_utf8_lossy(&self.seen)
                    )
                });
            self.seen
                .extend(self.trace.recv_timeout(remaining).unwrap());
        }
    }

    /// Continues the tracee a `signal=SIGSTOP` injection stopped.
    fn resume(&self) {
        let tracer = self.child.id();
        let tracee =
            std::fs::read_to_string(format!("/proc/{tracer}/task/{tracer}/children")).unwrap();
        let resumed = std::process::Command::new("kill")
            .arg("-CONT")
            .arg(tracee.trim())
            .status()
            .unwrap();
        assert!(resumed.success());
    }

    fn finish(self) -> std::process::ExitStatus {
        let mut child = self.child;
        let status = child.wait().unwrap();
        drop(self.trace);
        self.reader.join().unwrap();
        status
    }
}

#[cfg(target_os = "linux")]
#[test]
fn concurrent_syncs_attempt_an_unchanged_refusal_once() {
    // Given an authorized Ready task whose admission claim refuses its missing dependency.
    let mut f = super::queue_fixture();
    let task = f.task("app", Status::Ready);
    f.ctx
        .store
        .update(&task.id, |t| {
            t.depends_on = vec!["T-999".into()];
            Ok(())
        })
        .unwrap();
    f.grant(&[&task.id]);

    // When one sync process stops after opening the drain lease, before claiming it,
    // its competitor must commit the refusal before this process takes its fresh view.
    let refusals = f.ctx.store.dir().join("lock/queue-drain.lock");
    let refusals = refusals.to_str().unwrap();
    let stop = "inject=openat:signal=SIGSTOP:when=1";
    let mut stopped = Traced::sync(
        &f,
        &["strace", "-P", refusals, "-e", "trace=openat", "-e", stop],
    );
    stopped.until(|trace| trace.contains("stopped by SIGSTOP"));
    // another sync process runs to completion,
    f.ok(&["sync"]);
    // and the stopped one then resumes.
    stopped.resume();
    assert!(stopped.finish().success());

    // Then the unchanged refusal was attempted and logged exactly once.
    let failures = f
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "dispatch_failed" && e.task.as_deref() == Some(task.id.as_str()))
        .count();
    assert_eq!(failures, 1);
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

    // When GC (the exclusive holder) has artifact storage while a real sync process starts.
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
    while !String::from_utf8_lossy(&seen).contains("/artifact-storage.lock>, LOCK_SH") {
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

/// One Pi agent call's deadline (`agent/bridge/client.rs` IO_TIMEOUT).
#[cfg(target_os = "linux")]
const CALL_DEADLINE: Duration = Duration::from_secs(60);

/// A hook process: real `corral hook` invocations, each followed by a fresh artifact.
#[cfg(target_os = "linux")]
const HOOK_STREAM: &str = r#"set -u
corral=("$@")
k=0
while [ ! -e "$HOOK_DIR/stop" ] && [ "$k" -lt 200 ]; do
  k=$((k + 1))
  "${corral[@]}" hook 2>> "$HOOK_DIR/hook-$HOOK_N.err" || exit 1
  file="$HOOK_DIR/hook-$HOOK_N-$k.log"
  printf 'hook %s write %s\n' "$HOOK_N" "$k" > "$file"
  "${corral[@]}" artifact add "$HOOK_TASK" "$file" --kind log > /dev/null || exit 1
  echo WROTE
done
"#;

/// Kills fixture process groups on drop, so a failing assertion leaves none stopped or looping.
#[cfg(target_os = "linux")]
struct Groups(Vec<u32>);

#[cfg(target_os = "linux")]
impl Drop for Groups {
    fn drop(&mut self) {
        for group in &self.0 {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{group}")])
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

#[cfg(target_os = "linux")]
fn storage_busy(lock: &std::path::Path) -> bool {
    let file = std::fs::OpenOptions::new().write(true).open(lock).unwrap();
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

/// Runs `work` on a thread and requires it to finish within one agent-call deadline.
#[cfg(target_os = "linux")]
fn within<T: Send + 'static>(what: &str, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.recv_timeout(CALL_DEADLINE)
        .unwrap_or_else(|error| panic!("{what}: {error}"))
}

/// Awaits one more completed write from every hook process, bounded by the deadline.
#[cfg(target_os = "linux")]
fn writes_past(progress: &std::sync::mpsc::Receiver<usize>, writes: &mut [usize], what: &str) {
    let floor = writes.to_vec();
    let deadline = Instant::now() + CALL_DEADLINE;
    while writes.iter().zip(&floor).any(|(now, then)| now <= then) {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_else(|| panic!("{what}: {writes:?}"));
        let n = progress
            .recv_timeout(remaining)
            .unwrap_or_else(|error| panic!("{what}: {error} {writes:?}"));
        writes[n] += 1;
    }
}

#[cfg(target_os = "linux")]
#[test]
fn done_and_verdict_finish_while_hook_processes_keep_writing_artifacts() {
    use std::io::BufRead;
    use std::os::unix::process::CommandExt;
    const STREAMS: usize = 3;
    // Given a running task, a reviewed task, and tasks the hook processes attach to.
    let f = super::queue_fixture();
    let review = reviewed(&f, "other");
    let work = f.task("app", Status::Ready).id;
    f.ok(&["dispatch", &work]);
    let targets: Vec<String> = (0..=STREAMS)
        .map(|_| f.task("app", Status::Backlog).id)
        .collect();
    let lock = f.ctx.store.dir().join("lock/artifact-storage.lock");
    let path = lock.to_str().unwrap();
    let inject = "inject=flock:signal=SIGSTOP:when=1";
    let stop = ["strace", "-P", path, "-e", "trace=flock", "-e", inject];
    let stopped = |trace: &str| trace.contains("stopped by SIGSTOP");
    let mut groups = Groups(Vec::new());

    // When one hook's artifact write stops while it holds artifact storage,
    let source = f.temp.path().join("held-hook.log");
    std::fs::write(&source, "hook write held mid-flight\n").unwrap();
    let source = source.to_str().unwrap();
    let mut held = Traced::spawn(&f, &stop, &["artifact", "add", &targets[STREAMS], source]);
    groups.0.push(held.child.id());
    held.until(stopped);
    assert!(storage_busy(&lock));
    // and hook processes keep running `corral hook` and attaching fresh artifacts,
    let dir = f.temp.path().join("hooks");
    std::fs::create_dir(&dir).unwrap();
    let (tx, progress) = std::sync::mpsc::channel();
    let mut streams = Vec::new();
    for (n, target) in targets[..STREAMS].iter().enumerate() {
        let mut child = wrapped(&f, &["bash", "-c", HOOK_STREAM, "hook-stream"], &[])
            .env("HOOK_DIR", &dir)
            .env("HOOK_N", n.to_string())
            .env("HOOK_TASK", target)
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_SESSION")
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        groups.0.push(child.id());
        let stdout = child.stdout.take().unwrap();
        let tx = tx.clone();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                if line.unwrap() == "WROTE" && tx.send(n).is_err() {
                    break;
                }
            }
        });
        streams.push((child, reader));
    }
    drop(tx);
    let mut writes = vec![0; STREAMS];
    writes_past(&progress, &mut writes, "hooks stalled behind a held write");
    // and done and verdict each stop inside their own artifact-storage acquisition.
    f.checkpoint(&work);
    let summary = "Verified: fixture\nNot verified: none";
    let mut done = Traced::spawn(&f, &stop, &["done", &work, "--summary", summary]);
    let notes = "Reviewed while hooks write.";
    let mut verdict = Traced::spawn(
        &f,
        &stop,
        &["verdict", &review, "approve", "--notes", notes],
    );
    groups.0.extend([done.child.id(), verdict.child.id()]);
    done.until(stopped);
    verdict.until(stopped);

    // Then every hook process attaches another artifact while both calls are in flight,
    writes_past(&progress, &mut writes, "hooks stalled behind done/verdict");
    // and both calls finish within one agent-call deadline while a hook still holds storage.
    done.resume();
    verdict.resume();
    let finished = within("done/verdict missed the deadline", move || {
        (done.finish(), verdict.finish())
    });
    assert!(finished.0.success() && finished.1.success(), "{finished:?}");
    assert!(
        storage_busy(&lock),
        "the held hook write released storage early"
    );
    held.resume();
    assert!(held.finish().success());
    std::fs::write(dir.join("stop"), "").unwrap();
    for (mut child, reader) in streams {
        let status = within("a hook process did not stop", move || child.wait().unwrap());
        assert!(status.success(), "{status:?}");
        reader.join().unwrap();
    }
    for n in progress.try_iter() {
        writes[n] += 1;
    }
    for n in 0..STREAMS {
        let errors = std::fs::read_to_string(dir.join(format!("hook-{n}.err"))).unwrap();
        assert!(errors.is_empty(), "hook {n}: {errors}");
    }
    for (n, target) in targets[..STREAMS].iter().enumerate() {
        let attached = f.ctx.store.get(target).unwrap().artifacts.len();
        assert_eq!(attached, writes[n], "{target}");
    }
    assert_eq!(
        f.ctx.store.get(&targets[STREAMS]).unwrap().artifacts.len(),
        1
    );
    assert_eq!(f.ctx.store.get(&work).unwrap().status, Status::Review);
    let reviewed = f.ctx.store.get(&review).unwrap();
    let record = reviewed.current_run().unwrap().review.clone().unwrap();
    assert_eq!(record.verdict, Some(corral_core::Verdict::Approve));
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
