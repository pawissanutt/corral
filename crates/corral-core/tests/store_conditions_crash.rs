//! T-328: a condition recorded inside a task update commits in the update's own transaction.
//! A child increments `rounds` and observes or clears `queued` inside `update` or
//! `update_head`; the parent SIGKILLs it at a barrier inside that transaction or right after
//! its commit, and the reopened store must hold both the task change and the condition, or
//! neither. The child reports the barrier and blocks on stdin; no sleeps.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use corral_core::backend::OpKind;
use corral_core::sqlite::{SqliteBackend, SqliteOptions};
use corral_core::{Status, Store};
use serde_json::json;

const ROLE: &str = "CORRAL_CONDITION_CRASH_CHILD";

/// Re-executed as the child: record the condition inside the update and stop at the barrier.
#[test]
fn zz_crash_child() {
    let Ok(spec) = std::env::var(ROLE) else {
        return;
    };
    let [root, id, form, operation, point]: [&str; 5] =
        spec.split('|').collect::<Vec<_>>().try_into().unwrap();
    let target = point.to_owned();
    let base = Store::open(std::path::Path::new(root)).unwrap();
    let options = SqliteOptions {
        barrier: Some(Arc::new(move |barrier| {
            if barrier.op == OpKind::UpdateTask && barrier.what == target {
                println!("CRASH-BARRIER {}", barrier.what);
                std::io::stdout().flush().unwrap();
                let mut release = [0_u8];
                let _parent_closed = std::io::stdin().read(&mut release);
                std::process::abort();
            }
        })),
        ..SqliteOptions::default()
    };
    let store = base.with_backend(Box::new(SqliteBackend::open(base.dir(), options).unwrap()));
    let record = |store: &Store| -> anyhow::Result<()> {
        if operation == "clear" {
            store.clear_condition(Some(id), "queued")?;
        } else {
            store.log_event(Some(id), "queued", json!({"reason": "A"}))?;
        }
        Ok(())
    };
    if form == "head" {
        store
            .update_head(id, |task| {
                task.rounds += 1;
                record(&store)
            })
            .unwrap();
    } else {
        store
            .update(id, |task| {
                task.rounds += 1;
                record(&store)
            })
            .unwrap();
    }
    println!("CRASH-FINISHED");
}

/// Kills a child at `point` and returns (rounds, new events, runs, open runs) after reopening.
fn crash(form: &str, operation: &str, point: &str) -> (u32, u64, usize, usize) {
    // Given a disposable task and, for a clear, an already-open run.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let task = store
        .create("Atomic callback", None, Status::Ready, "")
        .unwrap();
    if operation == "clear" {
        store
            .log_event(Some(&task.id), "queued", json!({"reason": "A"}))
            .unwrap();
    }
    let before = store.event_tail().unwrap();
    drop(store);
    // When the child stops at the barrier and is killed there.
    let spec = format!(
        "{}|{}|{form}|{operation}|{point}",
        dir.path().display(),
        task.id
    );
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "zz_crash_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROLE, spec)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let line = line.unwrap_or_default();
            if let Some(at) = line.find("CRASH-") {
                let _ = send.send(line[at..].to_owned());
                return;
            }
        }
    });
    // A hang guard for the child's report, not a timing assertion.
    let reached = receive.recv_timeout(Duration::from_secs(60));
    child.kill().unwrap();
    let status = child.wait().unwrap();
    reader.join().unwrap();
    let reached = reached.expect("the child reported no barrier");
    assert_eq!(
        reached,
        format!("CRASH-BARRIER {point}"),
        "{form} {operation}"
    );
    assert!(
        !status.success(),
        "{form} {operation} {point}: child was not killed"
    );
    // Then the reopened store is read.
    let reopened = Store::open(dir.path()).unwrap();
    let rounds = reopened.get(&task.id).unwrap().rounds;
    let runs = reopened.condition_runs(Some(&task.id)).unwrap();
    let open = runs.iter().filter(|run| run.closed_seq.is_none()).count();
    let new_events = reopened.event_tail().unwrap() - before;
    (rounds, new_events, runs.len(), open)
}

#[test]
fn a_kill_inside_or_after_the_update_keeps_its_condition_with_its_task_change() {
    let mut report = Vec::new();
    for form in ["full", "head"] {
        for operation in ["observe", "clear"] {
            let inside = if operation == "clear" {
                "condition close"
            } else {
                "condition run"
            };
            for point in [inside, "committed"] {
                let (rounds, new_events, runs, open) = crash(form, operation, point);
                // Atomic: either nothing of the update, or the task change with its condition.
                let atomic = match (rounds, operation) {
                    (0, "observe") => new_events == 0 && runs == 0,
                    (0, _) => new_events == 0 && runs == 1 && open == 1,
                    (1, "observe") => new_events == 1 && runs == 1 && open == 1,
                    (1, _) => new_events == 1 && runs == 1 && open == 0,
                    _ => false,
                };
                let committed = rounds == 1;
                let line = format!(
                    "CRASH form={form} op={operation} at={point:?} rounds={rounds} \
                     new_events={new_events} runs={runs} open={open} atomic={atomic}"
                );
                println!("{line}");
                assert!(atomic, "{line}");
                assert_eq!(committed, point == "committed", "{line}");
                report.push(line);
            }
        }
    }
    assert_eq!(report.len(), 8);
}
