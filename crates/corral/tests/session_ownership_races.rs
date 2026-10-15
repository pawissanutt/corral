//! T-094: ownership established during engine IPC must win over stale preflights.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixDatagram;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use corral_core::{Status, Store, Task};
use corral_ops::SlotUsage;
use serde_json::Value;

#[path = "resource_support/mod.rs"]
mod resource_support;

const TIMEOUT: Duration = Duration::from_secs(30);

/// Own the whole CLI process group, including a fake engine blocked in recv().
/// Waiting happens on a completion channel, not a try_wait/sleep loop.
struct Process {
    pid: Option<u32>,
    completed: Receiver<std::io::Result<Output>>,
}

impl Process {
    fn spawn(mut command: Command) -> Self {
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        let (tx, completed) = mpsc::channel();
        std::thread::spawn(move || {
            // A timeout or panic kills this group before the receiver is dropped.
            let result = child.wait_with_output();
            if let Err(error) = tx.send(result) {
                eprintln!("race fixture completion receiver dropped: {error}");
            }
        });
        Self {
            pid: Some(pid),
            completed,
        }
    }

    fn finish(mut self) -> Output {
        let output = self
            .completed
            .recv_timeout(TIMEOUT)
            .expect("CLI did not exit after its engine barrier was released")
            .unwrap();
        self.pid = None;
        output
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(pid) = self.pid.take() {
            // No shell and no inherited engine selectors. Kill descendants as
            // well, so a panic cannot leave a fake engine or its pipes alive.
            match Command::new("/bin/kill")
                .env_clear()
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdin(Stdio::null())
                .output()
            {
                Ok(output) if output.status.success() => {}
                Ok(output) => eprintln!("race fixture group cleanup: {output:?}"),
                Err(error) => eprintln!("race fixture group cleanup: {error}"),
            }
            if let Err(error) = self.completed.recv_timeout(TIMEOUT) {
                eprintln!("race fixture completion during cleanup: {error}");
            }
        }
    }
}

struct Barrier {
    socket: UnixDatagram,
    path: PathBuf,
}

impl Barrier {
    fn new(root: &Path, name: &str) -> Self {
        let path = root.join(format!("{name}.sock"));
        let socket = UnixDatagram::bind(&path).unwrap();
        socket.set_read_timeout(Some(TIMEOUT)).unwrap();
        socket.set_write_timeout(Some(TIMEOUT)).unwrap();
        Self { socket, path }
    }

    fn pause(&self, mut command: Command, point: &str) -> Process {
        command
            .env("RACE_BARRIER", &self.path)
            .env("RACE_POINT", point);
        Process::spawn(command)
    }

    fn reached(&self, expected: &str) -> PathBuf {
        let mut bytes = [0; 128];
        let (len, from) = self
            .socket
            .recv_from(&mut bytes)
            .expect("fake engine did not reach the requested barrier");
        assert_eq!(std::str::from_utf8(&bytes[..len]).unwrap(), expected);
        from.as_pathname().unwrap().to_path_buf()
    }

    fn release(&self, reply: &Path) {
        assert_eq!(self.socket.send_to(b"release", reply).unwrap(), 7);
    }
}

struct Fixture {
    temp: tempfile::TempDir,
    store: Store,
    engine: PathBuf,
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    tasks: Value,
    events: Value,
    files: BTreeMap<PathBuf, Vec<u8>>,
    slots: SlotUsage,
}

impl Fixture {
    fn new() -> Self {
        // A short /tmp path also keeps Unix socket addresses below sun_path.
        let temp = tempfile::tempdir_in("/tmp").unwrap();
        let root = temp.path();
        for dir in ["app", "home", "config", "data", "state", "cache"] {
            std::fs::create_dir(root.join(dir)).unwrap();
        }
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.name", "ownership fixture"],
            vec!["config", "user.email", "ownership@example.invalid"],
            vec!["commit", "-q", "--allow-empty", "-m", "initial"],
        ] {
            let mut git = Self::clean_command(root, "git");
            git.arg("-C").arg(root.join("app")).args(args);
            assert_success(Process::spawn(git).finish());
        }
        let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let engine = root.join("herdr-fixture");
        std::fs::copy(tests.join("fixtures/session_ownership_herdr.py"), &engine).unwrap();
        std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(
            tests.join("queue_support/herdr.py"),
            root.join("queue-engine"),
        )
        .unwrap();
        let mut store = Store::init(root).unwrap();
        resource_support::configure(&mut store.config);
        store.config.defaults.profile = Some("generic".into());
        // Leave room for both competitors: ownership, not capacity, must refuse
        // the loser. No Ready admission or autopilot can launch incidental work.
        store.config.limits.running = 4;
        store.config.review.autopilot = Some(false);
        store.save_config().unwrap();
        Self {
            temp,
            store,
            engine,
        }
    }

    fn clean_command(root: &Path, bin: &str) -> Command {
        let mut command = Command::new(bin);
        // Drop all inherited CORRAL/HERDR identities, Git overrides and hooks.
        // Preserve only PATH for the fixture's Python interpreter and Git.
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(root)
            .stdin(Stdio::null());
        command
    }

    fn command(&self, session: &str, args: &[&str]) -> Command {
        let mut command = Self::clean_command(self.temp.path(), env!("CARGO_BIN_EXE_corral"));
        command
            .arg("--root")
            .arg(self.temp.path())
            .args(args)
            .env("CORRAL_ROOT", self.temp.path())
            .env("HERDR_BIN_PATH", &self.engine)
            .env("CORRAL_HERDR_SESSION", session)
            .env("HERDR_SESSION", session);
        command
    }

    fn ok(&self, session: &str, args: &[&str]) {
        assert_success(Process::spawn(self.command(session, args)).finish());
    }

    fn task(&self) -> Task {
        self.store
            .create("Ownership race", Some("app"), Status::Ready, "")
            .unwrap()
    }

    fn socket(&self, session: &str) -> PathBuf {
        self.temp
            .path()
            .join("config/herdr/sessions")
            .join(session)
            .join("herdr.sock")
    }

    fn snapshot(&self) -> Snapshot {
        let store = Store::open_unobserved(self.temp.path()).unwrap();
        let tasks = store.list().unwrap();
        Snapshot {
            slots: SlotUsage::from_tasks(&tasks, &store.config.limits),
            tasks: serde_json::to_value(tasks).unwrap(),
            events: serde_json::to_value(store.events().unwrap()).unwrap(),
            // Include caches, observations, nested state, events and raw task
            // bytes. Only advisory lock-file creation is not a store mutation.
            files: snapshot_files(store.dir()),
        }
    }

    fn assert_owner_run(&self, id: &str) {
        let task = self.store.get(id).unwrap();
        assert_eq!(task.status, Status::Running);
        assert!(task.launch.is_none());
        assert_eq!(task.runs.len(), 1);
        let reference = task.current_run().unwrap().herdr.as_ref().unwrap();
        assert_eq!(reference.pane_id, "w1:p1");
        assert_eq!(reference.socket.as_ref(), Some(&self.socket("owner")));
        let usage = self.snapshot().slots;
        assert_eq!(usage.total(), 1);
        assert_eq!(usage.reservations, 0);
    }

    fn assert_unlaunched(&self, id: &str) {
        let task = self.store.get(id).unwrap();
        assert_eq!(task.status, Status::Ready);
        assert!(task.runs.is_empty());
        assert!(task.launch.is_none());
        assert!(task.dispatch_plan.is_none());
        assert!(!task.queued);
        assert!(!self.temp.path().join("app/.worktrees").join(id).exists());
    }
}

fn snapshot_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap() == "lock" && path.is_dir() {
            continue;
        }
        if path.is_dir() {
            files.extend(snapshot_files(&path));
        } else {
            files.insert(path.clone(), std::fs::read(path).unwrap());
        }
    }
    files
}

fn assert_success(output: Output) {
    assert!(output.status.success(), "{output:?}");
}

fn foreign_inventory_after_first_dispatch(verb: &str, retargeted: bool) {
    // Given an empty, unbound store and a foreign CLI past its ownership checks.
    let fixture = Fixture::new();
    let task = fixture.task();
    assert!(fixture.store.herdr_session_binding().unwrap().is_none());
    assert!(task.runs.is_empty());
    let barrier = Barrier::new(fixture.temp.path(), "inventory");
    let selected = if retargeted { "owner" } else { "foreign" };
    let mut command = fixture.command(selected, &[verb]);
    if retargeted {
        command
            .env("HERDR_SESSION", "foreign")
            .env("RACE_FOREIGN_SOURCE", "1");
    }
    let foreign = barrier.pause(command, "pane list");
    let reply = barrier.reached(&format!("{selected}:pane list"));

    // When owner dispatch establishes w1:p1 ownership before the blocked
    // foreign response with that same ID is allowed to reach reconciliation.
    fixture.ok("owner", &["dispatch", &task.id]);
    fixture.assert_owner_run(&task.id);
    let after_dispatch = fixture.snapshot();
    barrier.release(&reply);
    let output = foreign.finish();

    // Then even observation/cache writes and slot changes are forbidden.
    assert_success_output_is_silent(&output);
    assert_eq!(fixture.snapshot(), after_dispatch);
    fixture.assert_owner_run(&task.id);
}

fn assert_success_output_is_silent(output: &Output) {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn foreign_hook_inventory_cannot_reconcile_concurrent_first_dispatch() {
    foreign_inventory_after_first_dispatch("hook", false);
}

#[test]
fn foreign_sync_inventory_cannot_reconcile_concurrent_first_dispatch() {
    foreign_inventory_after_first_dispatch("sync", false);
}

#[test]
fn retargeted_foreign_source_hook_cannot_reconcile_concurrent_first_dispatch() {
    foreign_inventory_after_first_dispatch("hook", true);
}

#[test]
fn foreign_dispatch_cannot_claim_after_concurrent_session_binding() {
    // Given a foreign dispatch stopped after preflight but before board admission.
    let fixture = Fixture::new();
    let task = fixture.task();
    let barrier = Barrier::new(fixture.temp.path(), "binding");
    let foreign = barrier.pause(
        fixture.command("foreign", &["dispatch", &task.id]),
        "workspace list",
    );
    let reply = barrier.reached("foreign:workspace list");
    fixture.assert_unlaunched(&task.id);

    // When the actual bind-session CLI binds the empty store to another socket.
    let evidence = fixture.temp.path().join("original-launch.txt");
    std::fs::write(&evidence, "Fixture owner launch registration\n").unwrap();
    fixture.ok(
        "owner",
        &[
            "bind-session",
            "--socket",
            fixture.socket("owner").to_str().unwrap(),
            "--launch-record",
            evidence.to_str().unwrap(),
        ],
    );
    assert_eq!(
        fixture
            .store
            .herdr_session_binding()
            .unwrap()
            .unwrap()
            .socket,
        fixture.socket("owner")
    );
    let after_binding = fixture.snapshot();
    assert_eq!(after_binding.slots.total(), 0);
    barrier.release(&reply);
    let output = foreign.finish();

    // Then refusal cannot leave even a released reservation or an error event.
    assert!(!output.status.success(), "{output:?}");
    assert!(!output.stderr.is_empty(), "{output:?}");
    assert_eq!(fixture.snapshot(), after_binding);
    fixture.assert_unlaunched(&task.id);
}

#[test]
fn competing_first_dispatches_on_distinct_sockets_cannot_mix_ownership() {
    // Given distinct task locks, both dispatches have passed ownership preflight
    // on the same empty board. Four slots rule out a capacity-only refusal.
    let fixture = Fixture::new();
    let owner_task = fixture.task();
    let foreign_task = fixture.task();
    let owner_barrier = Barrier::new(fixture.temp.path(), "owner");
    let foreign_barrier = Barrier::new(fixture.temp.path(), "foreign");
    let owner = owner_barrier.pause(
        fixture.command("owner", &["dispatch", &owner_task.id]),
        "workspace list",
    );
    let owner_reply = owner_barrier.reached("owner:workspace list");
    let foreign = foreign_barrier.pause(
        fixture.command("foreign", &["dispatch", &foreign_task.id]),
        "workspace list",
    );
    let foreign_reply = foreign_barrier.reached("foreign:workspace list");
    fixture.assert_unlaunched(&owner_task.id);
    fixture.assert_unlaunched(&foreign_task.id);

    // When the owner wins admission, then the stale foreign preflight resumes.
    owner_barrier.release(&owner_reply);
    assert_success(owner.finish());
    fixture.assert_owner_run(&owner_task.id);
    let after_winner = fixture.snapshot();
    foreign_barrier.release(&foreign_reply);
    let output = foreign.finish();

    // Then the losing task, events, caches and capacity remain exactly unchanged.
    assert!(!output.status.success(), "{output:?}");
    assert!(!output.stderr.is_empty(), "{output:?}");
    assert_eq!(fixture.snapshot(), after_winner);
    fixture.assert_owner_run(&owner_task.id);
    fixture.assert_unlaunched(&foreign_task.id);
    let engine: Value =
        serde_json::from_slice(&std::fs::read(fixture.temp.path().join("protocol.json")).unwrap())
            .unwrap();
    assert_eq!(engine["creates"], 1);
    assert_eq!(engine["submits"], 1);
}
