//! Artifact-storage contention between hook passes and finishing calls (T-659).
use super::*;

use std::collections::BTreeSet;
use std::process::Stdio;

/// One Pi agent call's deadline (`agent/bridge/client.rs` IO_TIMEOUT).
const CALL_DEADLINE: Duration = Duration::from_secs(60);
const HOOK_WRITERS: usize = 4;
const ACQUIRED: &str = "artifact_storage_held";
/// A pass's task read, artifact event, index build and PM event push.
const PHASES: [&str; 4] = [
    "sync_tasks_read",
    "artifact_event",
    "content_index_build",
    "sync_events",
];

#[test]
fn done_and_verdict_finish_while_hook_writers_hold_artifact_storage() {
    // Given a running task, a task in review, and hook writers attaching to their own tasks.
    let fx = Fixture::new(2);
    let review = fx.task();
    let work = fx.task();
    fx.ctx
        .store
        .update(&work, |t| {
            t.status = Status::Running;
            Ok(())
        })
        .unwrap();
    let (inside_tx, inside) = mpsc::channel();
    let mut releases = Vec::new();
    let mut writers = Vec::new();
    for n in 0..HOOK_WRITERS {
        let (ctx, id) = (fx.ctx.clone(), fx.task());
        let source = fx.tmp.path().join(format!("hook-{n}.log"));
        std::fs::write(&source, format!("hook writer {n}\n")).unwrap();
        let (release, released) = mpsc::channel::<()>();
        releases.push(release);
        let inside_tx = inside_tx.clone();
        writers.push(std::thread::spawn(move || {
            let mut parked = false;
            crate::agent::FAILPOINT.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move |point| {
                    if point != "artifact_storage_held" || std::mem::replace(&mut parked, true) {
                        return;
                    }
                    inside_tx.send(n).unwrap();
                    // Parked mid-write until released; bounded so a failure cannot hang it.
                    let _ = released.recv_timeout(CALL_DEADLINE * 2);
                }));
            });
            crate::artifacts::add_artifact(&ctx, &id, &source, None, Some("hook log"))
        }));
    }

    // When a hook writer is parked inside artifact storage and verdict and done start.
    let first = inside
        .recv_timeout(CALL_DEADLINE)
        .expect("no hook writer reached artifact storage");
    let (ctx, verdict_id, done_id) = (fx.ctx.clone(), review.clone(), work.clone());
    let (finished_tx, finished) = mpsc::channel();
    let caller = std::thread::spawn(move || {
        let notes = "Reviewed while hooks write.";
        let verdict = crate::reviewer::verdict(&ctx, &verdict_id, Verdict::Approve, notes);
        let done = crate::done(&ctx, &done_id, "Verified: fixture\nNot verified: none");
        finished_tx
            .send((verdict.map(drop), done.map(drop)))
            .unwrap();
    });

    // Then both finish within one agent-call deadline while that writer still holds storage,
    let (verdict, done) = finished
        .recv_timeout(CALL_DEADLINE)
        .expect("verdict/done starved behind a hook writer holding artifact storage");
    verdict.unwrap();
    done.unwrap();
    // and every hook writer is inside storage at once, none released before this point.
    let mut holding = BTreeSet::from([first]);
    while holding.len() < HOOK_WRITERS {
        let n = inside
            .recv_timeout(CALL_DEADLINE)
            .expect("hook writers serialized on artifact storage");
        holding.insert(n);
    }
    for release in releases {
        release.send(()).unwrap();
    }
    for writer in writers {
        writer.join().unwrap().unwrap();
    }
    caller.join().unwrap();
    assert_eq!(fx.ctx.store.get(&work).unwrap().status, Status::Review);
    let reviewed = fx.ctx.store.get(&review).unwrap();
    let record = reviewed.current_run().unwrap().review.clone().unwrap();
    assert_eq!(record.verdict, Some(Verdict::Approve));
}

#[test]
fn hook_pass_holds_artifact_storage_only_around_artifact_writes() {
    // Given a finished gate the pass must collect and attach,
    let fx = Fixture::new(2);
    let id = fx.task();
    crate::gate::gate_start(&fx.ctx, &id).unwrap();
    let script = crate::gate::gate_paths(&fx.ctx, &id, "R1").script;
    let ran = support::command("bash", fx.tmp.path())
        .arg(&script)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(ran.status.success(), "{ran:?}");
    // and a herdr that records whether artifact storage is free at each call: its calls span
    // inventory, task reconciliation, pane cleanup and launches, the pass's non-artifact phases.
    let lock = fx.ctx.store.dir().join("lock/artifact-storage.lock");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let herdr = fx.tmp.path().join("herdr");
    std::fs::rename(&herdr, fx.tmp.path().join("herdr-fake")).unwrap();
    let wrapper = format!(
        r#"#!/usr/bin/env bash
dir="$(cd "$(dirname "$0")" && pwd)"
if /usr/bin/flock -n '{}' true; then state=free; else state=held; fi
printf '%s %s\n' "$state" "$*" >> "$dir/herdr-calls.log"
exec "$dir/herdr-fake" "$@"
"#,
        lock.display()
    );
    std::fs::write(&herdr, wrapper).unwrap();
    std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Each acquisition and each read, event and index phase on this thread records whether
    // storage is held, through another handle and by this thread.
    let probes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let (seen, probe) = (probes.clone(), lock.clone());
    crate::agent::FAILPOINT.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |point| {
            if point != ACQUIRED && !PHASES.contains(&point) {
                return;
            }
            let other = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&probe)
                .unwrap();
            let held = matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock));
            let here = crate::artifact_storage::held_here();
            seen.borrow_mut().push((point.to_owned(), held, here));
        }));
    });

    // When a hook's sync pass runs.
    let synced = crate::sync_at(&fx.ctx, Timestamp::now());
    crate::agent::FAILPOINT.with(|slot| slot.borrow_mut().take());
    synced.unwrap();

    // Then every herdr call ran with artifact storage free,
    let calls = std::fs::read_to_string(fx.tmp.path().join("herdr-calls.log")).unwrap();
    assert!(calls.lines().any(|l| l.contains("pane list")), "{calls}");
    assert!(calls.lines().all(|l| l.starts_with("free ")), "{calls}");
    // every read, event and index phase ran with it free, on this thread and to other handles,
    let probes = probes.borrow();
    for phase in PHASES {
        assert!(
            probes.iter().any(|(p, ..)| p == phase),
            "{phase}: {probes:?}"
        );
    }
    let phases = probes.iter().filter(|(p, ..)| p != ACQUIRED);
    assert!(
        phases.clone().all(|(_, held, here)| !held && !here),
        "{probes:?}"
    );
    // and the gate's artifacts were written under storage that was really held.
    let acquired: Vec<_> = probes.iter().filter(|(p, ..)| p == ACQUIRED).collect();
    assert!(
        !acquired.is_empty() && acquired.iter().all(|(_, held, _)| *held),
        "{probes:?}"
    );
    let task = fx.ctx.store.get(&id).unwrap();
    let gate = task.current_run().unwrap().gate.clone().unwrap();
    assert_eq!(gate.result, corral_core::GateResult::Pass);
    assert!(task.artifacts.iter().any(|a| a.run == "R1"), "{task:?}");
}
