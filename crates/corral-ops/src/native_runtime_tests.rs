//! Native fixtures use this test executable, not shebangs or an installed interpreter.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use corral_core::StuckProcess;

use crate::hook_process::HookChild;
use crate::{ProcessControl, SystemProcesses};

const FIXTURE: &str = "native_runtime_tests::native_fixture_process";
const BOUND: Duration = Duration::from_secs(30);

#[cfg(windows)]
#[test]
fn native_harness_protection_preserves_windows_case_and_suffix_aliases() {
    // Given a configured native harness and an internal interpreter-hosted tool.
    let root = tempfile::tempdir().unwrap();
    let mut ctx = crate::Ctx::from_store(corral_core::Store::init(root.path()).unwrap());
    ctx.store.config.agents.insert(
        "custom".into(),
        corral_core::AgentCmd {
            command: r"C:\tools\CustomHarness.EXE".into(),
        },
    );
    let run: corral_core::Run = serde_json::from_value(serde_json::json!({
        "id":"R1", "agent":"custom", "worktree":root.path(), "branch":"corral/T-001",
        "base":"main", "base_sha":"abc", "started":"2026-10-17T00:00:00Z"
    }))
    .unwrap();
    // When process inventory returns Windows path casing and executable suffixes.
    let harness = crate::watchdog::protected(&ctx, &run, &[r"C:\TOOLS\CUSTOMHARNESS.exe".into()]);
    let internal = crate::watchdog::protected(
        &ctx,
        &run,
        &[
            r"C:\tools\BUN.EXE".into(),
            r"C:\node_modules\@Code-Yeongyu\Senpi\dist\cli.js".into(),
        ],
    );
    // Then neither legitimate harness identity can be selected for termination.
    assert!(harness);
    assert!(internal);
}

fn fixture(mode: &str, address: &str, cwd: &std::path::Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", FIXTURE, "--nocapture"])
        .env("CORRAL_NATIVE_FIXTURE", mode)
        .env("CORRAL_NATIVE_ADDRESS", address)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[test]
fn native_fixture_process() {
    let Ok(mode) = std::env::var("CORRAL_NATIVE_FIXTURE") else {
        return;
    };
    let address = std::env::var("CORRAL_NATIVE_ADDRESS").unwrap();
    let descendant = if mode == "parent" {
        // Keep the native group/job inherited by the descendant.
        Some(ChildGuard(
            fixture("leaf", &address, &std::env::current_dir().unwrap())
                .spawn()
                .unwrap(),
        ))
    } else {
        None
    };
    let mut socket = TcpStream::connect_timeout(&address.parse().unwrap(), BOUND).unwrap();
    socket.set_read_timeout(Some(BOUND)).unwrap();
    writeln!(socket, "{mode} {}", std::process::id()).unwrap();
    socket.read_exact(&mut [0]).unwrap();
    if descendant.is_some() {
        // Deliberately exit without dropping the guard: this fixture tests that
        // the production group/job contains descendants after their parent exits.
        // Before this signal, the guard still cleans up if fixture setup fails.
        std::process::exit(0);
    }
}

type Ready = (String, u32, TcpStream);

fn assert_closed(stream: &mut TcpStream) {
    let result = stream.read(&mut [0]);
    #[cfg(windows)]
    assert!(
        matches!(result, Ok(0))
            || result.is_err_and(|e| e.kind() == std::io::ErrorKind::ConnectionReset)
    );
    #[cfg(not(windows))]
    assert_eq!(result.unwrap(), 0);
}

fn listen(count: usize) -> (String, Receiver<Ready>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let (tx, rx) = mpsc::channel();
    // Subscribe before spawning. The only timeout guards a failed fixture handshake.
    std::thread::spawn(move || {
        for _ in 0..count {
            let (stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(BOUND)).unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let (kind, pid) = line.trim().split_once(' ').unwrap();
            tx.send((kind.into(), pid.parse().unwrap(), stream))
                .unwrap();
        }
    });
    (address, rx)
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}

#[test]
fn native_termination_checks_identity_and_preserves_other_processes() {
    // Given two ready native processes, with no external shell or Python dependency.
    let root = tempfile::tempdir().unwrap();
    let (address, ready) = listen(2);
    let mut target = ChildGuard(fixture("leaf", &address, root.path()).spawn().unwrap());
    let mut peer = ChildGuard(fixture("leaf", &address, root.path()).spawn().unwrap());
    let _first = ready.recv_timeout(BOUND).unwrap();
    let _second = ready.recv_timeout(BOUND).unwrap();
    let snapshot = SystemProcesses.inspect(target.0.id()).unwrap().unwrap();
    let mut identity = StuckProcess {
        pid: target.0.id(),
        started: snapshot.started.clone(),
        argv: snapshot.argv.clone(),
    };
    // When recovery receives a stale start identity or changed argv.
    identity.started.push_str("-stale");
    assert!(SystemProcesses.terminate(&identity).is_err());
    identity.started = snapshot.started;
    identity.argv.push("changed".into());
    assert!(SystemProcesses.terminate(&identity).is_err());
    assert!(target.0.try_wait().unwrap().is_none());
    identity.argv = snapshot.argv;
    SystemProcesses.terminate(&identity).unwrap();
    // Then only the confirmed native identity exits.
    assert!(!target.0.wait().unwrap().success());
    assert!(peer.0.try_wait().unwrap().is_none());
}

#[test]
fn native_hook_completion_stops_descendants_before_receipting() {
    // Given a hook whose direct child exits successfully while a descendant remains.
    let root = tempfile::tempdir().unwrap();
    let (address, ready) = listen(2);
    let child = HookChild::spawn(fixture("parent", &address, root.path())).unwrap();
    let mut streams = [
        ready.recv_timeout(BOUND).unwrap(),
        ready.recv_timeout(BOUND).unwrap(),
    ];
    let parent = streams
        .iter_mut()
        .find(|(mode, _, _)| mode == "parent")
        .unwrap();
    // When the direct process is released and the production runner completes.
    parent.2.write_all(b"!").unwrap();
    let (status, timed_out) = child.wait(30).unwrap();
    // Then success is retained, but the descendant cannot outlive the receipt.
    assert!(status.success());
    assert!(!timed_out);
    let leaf = streams
        .iter_mut()
        .find(|(mode, _, _)| mode == "leaf")
        .unwrap();
    assert_closed(&mut leaf.2);
}

#[test]
fn native_hook_timeout_is_bounded_and_stops_the_selected_job() {
    // Given a ready command blocked on a socket, not on elapsed sleeps.
    let root = tempfile::tempdir().unwrap();
    let (address, ready) = listen(1);
    let child = HookChild::spawn(fixture("leaf", &address, root.path())).unwrap();
    let (_, _, mut stream) = ready.recv_timeout(BOUND).unwrap();
    // When its deadline is already expired.
    let (status, timed_out) = child.wait(0).unwrap();
    // Then the native process/job has exited and the timeout is explicit.
    assert!(timed_out);
    assert!(!status.success());
    assert_closed(&mut stream);
}

#[test]
fn native_hook_execution_persists_a_complete_receipt() {
    // Given a configured native argv hook, with file-backed output and a private store.
    let root = tempfile::tempdir().unwrap();
    let ctx = crate::Ctx::from_store(corral_core::Store::init(root.path()).unwrap());
    let (address, ready) = listen(1);
    let mut environment: std::collections::BTreeMap<_, _> = std::env::vars().collect();
    environment.insert("CORRAL_NATIVE_FIXTURE".into(), "once".into());
    environment.insert("CORRAL_NATIVE_ADDRESS".into(), address);
    let op: crate::PostLandOperation = serde_json::from_value(serde_json::json!({
        "version":1, "key":"native-hook", "project":"app",
        "repo":root.path().canonicalize().unwrap(),
        "task":"T-001", "run":"R1", "base":"main", "base_before":"before",
        "landed_sha":"after", "policy_digest":"native-policy",
        "created":"2026-10-17T00:00:00Z", "landed":true, "abandoned":false,
        "notified":false, "hooks":[{
            "hook": {
                "name":"native", "argv":[std::env::current_exe().unwrap(), "--exact", FIXTURE, "--nocapture"],
                "cwd":".", "env":environment, "timeout_secs":30, "max_attempts":1, "identity":null
            }, "state":"pending", "attempts":[], "reason":null
        }]
    })).unwrap();
    let run = std::thread::spawn(move || {
        let mut op = op;
        crate::hook_command::execute(&ctx, &mut op, 0).unwrap();
        let saved = crate::post_land_operations(&ctx, None).unwrap();
        (op, saved)
    });
    let (_, _, mut socket) = ready.recv_timeout(BOUND).unwrap();
    // When the real hook completes after its readiness signal.
    socket.write_all(b"!").unwrap();
    let (op, saved) = run.join().unwrap();
    // Then durable state includes the actual native exit and complete file-backed log.
    assert_eq!(saved.len(), 1);
    assert_eq!(op.hooks[0].state, crate::HookState::Succeeded);
    assert_eq!(saved[0].hooks[0].state, crate::HookState::Succeeded);
    let attempt = &saved[0].hooks[0].attempts[0];
    assert_eq!(attempt.exit_code, Some(0));
    assert!(!attempt.timed_out);
    assert!(attempt.finished.is_some());
    assert!(std::fs::metadata(&attempt.log).unwrap().len() > 0);
}

#[test]
fn native_artifact_reader_rejects_a_nonregular_replacement() {
    // Given an immutable artifact snapshot on the native filesystem.
    let root = tempfile::tempdir().unwrap();
    let ctx = crate::Ctx::from_store(corral_core::Store::init(root.path()).unwrap());
    let task = ctx
        .store
        .create("Native artifact", None, corral_core::Status::Backlog, "")
        .unwrap();
    let source = root.path().join("native.txt");
    std::fs::write(&source, b"native artifact bytes").unwrap();
    let artifact = crate::add_artifact(&ctx, &task.id, &source, None, None).unwrap();
    assert_eq!(
        crate::artifact_read(&ctx, &artifact).unwrap(),
        b"native artifact bytes"
    );
    assert_eq!(
        crate::artifact_storage::artifact_read_confined(&ctx, &artifact).unwrap(),
        b"native artifact bytes"
    );
    let snapshot = ctx.store.dir().join(&artifact.path);
    std::fs::remove_file(&snapshot).unwrap();
    std::fs::create_dir(&snapshot).unwrap();
    // When a retained snapshot path is replaced with a directory.
    let read = crate::artifact_read(&ctx, &artifact);
    // Then it fails before trying to consume nonregular content.
    assert!(read.is_err());
    assert!(crate::artifact_storage::artifact_read_confined(&ctx, &artifact).is_err());
}

#[cfg(not(target_os = "linux"))]
#[test]
fn native_inventory_finds_worktree_residents_and_hook_tokens() {
    // Given a native command running inside a disposable worktree path.
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let (address, ready) = listen(1);
    let token = format!("native-fixture-{}", std::process::id());
    let child = ChildGuard(
        fixture("leaf", &address, &path)
            .env("CORRAL_HOOK_TOKEN", &token)
            .spawn()
            .unwrap(),
    );
    let _ready = ready.recv_timeout(BOUND).unwrap();
    // When inventory is queried through the production native boundary.
    let residents = crate::native_process::residents(&path).unwrap();
    let token_alive = crate::native_process::has_token(&token).unwrap();
    // Then both cwd membership and interrupted-hook ownership are observable.
    assert!(residents.iter().any(|process| process.pid == child.0.id()));
    assert!(token_alive);
}
