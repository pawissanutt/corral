//! Shared settlement derivation is serialized before reading its versioned cursor.
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Instant;

use super::{CALL_DEADLINE, Groups, within, wrapped};

#[test]
fn concurrent_hooks_read_settlement_state_only_after_its_derivation_lease() {
    let f = super::super::queue_fixture();
    f.ok(&["hook"]);
    let lock = f
        .ctx
        .store
        .cache_root()
        .unwrap()
        .with_extension("terminal-settlement.lock");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let held = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock)
        .unwrap();
    held.lock().unwrap();
    let mut groups = Groups(Vec::new());
    let mut hooks = Vec::new();
    // Concurrent hooks coalesce into one sync, so the second contender is a plain sync.
    for args in [&["hook"][..], &["sync"][..]] {
        let mut child = wrapped(&f, &["strace", "-yy", "-e", "trace=flock"], args)
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        groups.0.push(child.id());
        let mut trace = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut chunk = [0; 4096];
            while let Ok(n) = trace.read(&mut chunk) {
                if n == 0 || tx.send(chunk[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + CALL_DEADLINE;
        let mut seen = Vec::new();
        let entry = format!("{}>, LOCK_EX", lock.display());
        while !String::from_utf8_lossy(&seen).contains(&entry) {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("hook did not reach settlement lease");
            seen.extend(rx.recv_timeout(remaining).unwrap_or_else(|error| {
                panic!(
                    "hook missed settlement lease: {error}; {}",
                    String::from_utf8_lossy(&seen)
                )
            }));
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "hook bypassed held lease"
        );
        hooks.push((child, reader, rx));
    }

    // Publish a new cursor while both hooks are blocked. They must read this version,
    // not replace it from a stale pre-lock read or lose a competing writer's update.
    f.ctx
        .store
        .log_event(None, "settlement_fixture", serde_json::json!({}))
        .unwrap();
    let seq = f.ctx.store.view().unwrap().seq().0;
    f.ctx
        .store
        .save_record(
            "terminal-settlement",
            "state",
            &serde_json::json!({
                "cursor": format!("{seq:020}"), "tasks": []
            }),
        )
        .unwrap();
    held.unlock().unwrap();
    for (mut child, reader, rx) in hooks {
        let status = within("hook failed to finish after lease release", move || {
            child.wait().unwrap()
        });
        assert!(status.success(), "hook failed after lease release");
        reader.join().unwrap();
        let trace: Vec<u8> = rx.try_iter().flatten().collect();
        assert!(!String::from_utf8_lossy(&trace).contains("concurrent write refused"));
    }
    let state: serde_json::Value = f
        .ctx
        .store
        .record("terminal-settlement", "state")
        .unwrap()
        .unwrap();
    assert!(state["cursor"].as_str().unwrap().parse::<u64>().unwrap() >= seq);
    assert_eq!(state["tasks"], serde_json::json!([]));
}
