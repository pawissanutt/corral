//! Exercise the real watch loop across an atomic executable replacement.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

struct Watch(Child);

impl Drop for Watch {
    fn drop(&mut self) {
        if let Err(error) = self.0.kill() {
            eprintln!("stopping watch fixture: {error}");
        }
        if let Err(error) = self.0.wait() {
            eprintln!("reaping watch fixture: {error}");
        }
    }
}

#[test]
fn watch_refuses_next_tick_after_its_executable_is_replaced() {
    // Given: a copied real binary and a separate fake terminal, never the live store.
    let tmp = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(tmp.path()).unwrap();
    let binary = tmp.path().join("corral");
    std::fs::copy(env!("CARGO_BIN_EXE_corral"), &binary).unwrap();
    let fifo = tmp.path().join("ready");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .stdin(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    let engine = tmp.path().join("herdr");
    std::os::unix::fs::symlink(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/version_skew/herdr.sh"),
        &engine,
    )
    .unwrap();
    // Subscribe before launch; generous guards bound hangs, not scheduler latency.
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut byte = [0_u8];
        let result = std::fs::File::open(fifo).and_then(|mut f| f.read_exact(&mut byte));
        ready_tx.send(result).unwrap();
    });
    let mut watch = Watch(
        command(&binary, tmp.path())
            .args(["--root", tmp.path().to_str().unwrap(), "sync", "--watch"])
            .current_dir(tmp.path())
            .env("HERDR_BIN_PATH", &engine)
            .env("CORRAL_HERDR_SESSION", "corral-test")
            .env("CORRAL_READY_PIPE", tmp.path().join("ready"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stderr = watch.0.stderr.take().unwrap();
    let (updated_tx, updated_rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let line = line.unwrap();
            if line.contains(corral_core::UPDATE_NOTICE) {
                updated_tx.send(line).unwrap();
                return;
            }
        }
    });
    ready_rx
        .recv_timeout(Duration::from_secs(120))
        .unwrap()
        .unwrap();
    // When: install a new inode while the watch process holds the old executable.
    let replacement = tmp.path().join("replacement");
    std::fs::copy(env!("CARGO_BIN_EXE_corral"), &replacement).unwrap();
    std::fs::rename(replacement, &binary).unwrap();
    // Then: the actual next tick reports skew and persists the event.
    let notice = updated_rx.recv_timeout(Duration::from_secs(120)).unwrap();
    println!("{notice}");
    let events = store.events().unwrap();
    assert_eq!(
        events.iter().filter(|e| e.kind == "version_skew").count(),
        1
    );
    assert!(!events.iter().any(|e| matches!(
        e.kind.as_str(),
        "gate_started" | "review_started" | "dispatched" | "gate"
    )));
}

#[test]
fn latched_process_resumes_sync_through_the_installed_executable() {
    // Given: a store whose scheduled digest every completed sync pass writes, and a process
    // latched by an update whose cause is gone again (T-587); never the live store or herdr.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = corral_core::Store::init(tmp.path()).unwrap();
    store.config.digest.every = Some("1h".into());
    store.save_config().unwrap();
    let ctx = corral_ops::Ctx {
        store,
        herdr: corral_herdr::Herdr::with_bin(
            tmp.path().join("no-herdr"),
            Some("corral-test".into()),
        ),
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
    };
    let marker = ctx.store.state_dir().join("schema-version");
    let current = std::fs::read(&marker).ok();
    std::fs::write(&marker, "999\n").unwrap();
    let refused = corral_ops::sync(&ctx).unwrap_err();
    assert_eq!(refused.to_string(), corral_core::UPDATE_NOTICE);
    match current {
        Some(bytes) => std::fs::write(&marker, bytes).unwrap(),
        None => std::fs::remove_file(&marker).unwrap(),
    }
    let count = |kind: &str| {
        let events = ctx.store.events().unwrap();
        events.iter().filter(|event| event.kind == kind).count()
    };
    // The stall: every later in-process pass refuses, so nothing advances.
    let refused = corral_ops::sync(&ctx).unwrap_err();
    assert_eq!(refused.to_string(), corral_core::UPDATE_NOTICE);
    assert_eq!(count("digest"), 0);

    // When: the installed executable runs the latched process's next pass.
    corral_ops::sync_installed(&ctx, std::path::Path::new(env!("CARGO_BIN_EXE_corral"))).unwrap();

    // Then: the pass completed its side effects; the latched process wrote only its skew event.
    assert_eq!(count("digest"), 1);
    assert_eq!(count("version_skew"), 1);
    assert!(ctx.store.runtime_latched());
}

#[test]
fn inherited_gate_attempt_cannot_discover_parent_workspace() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(tmp.path()).unwrap();
    store
        .create("Must stay isolated", None, corral_core::Status::Ready, "")
        .unwrap();
    let nested = tmp.path().join(".worktrees/T-001/crates/test");
    std::fs::create_dir_all(&nested).unwrap();
    // When: a test starts the installed CLI with the enclosing gate environment.
    let output = command(env!("CARGO_BIN_EXE_corral"), tmp.path())
        .args(["list", "--json"])
        .current_dir(&nested)
        .env_remove("CORRAL_ROOT")
        .env("CORRAL_GATE_ATTEMPT", "enclosing-real-attempt")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    // Then
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(store.list().unwrap().len(), 1);
    println!("{}", String::from_utf8_lossy(&output.stderr));
}

fn command(binary: impl AsRef<std::ffi::OsStr>, root: &std::path::Path) -> Command {
    let mut command = Command::new(binary);
    command.env("CORRAL_ROOT", root);
    for name in [
        "CORRAL_GATE_ATTEMPT",
        "CORRAL_CANDIDATE_HEAD",
        "CORRAL_INTEGRATION_BASE",
        "HERDR_PANE_ID",
        "HERDR_WORKSPACE_ID",
        "HERDR_TAB_ID",
    ] {
        command.env_remove(name);
    }
    command
}
