//! T-328 slim-events: the five noise kinds are condition runs (plan T-253 section 9).

use std::process::{Command, Stdio};

use corral_core::backend::{CONDITION_KINDS, CloseReason, ConditionOutcome, ConditionRun};
use corral_core::{Actor, Event, Status, Store};
use serde_json::{Value, json};

const CHILD: &str = "CORRAL_CONDITIONS_CHILD_KIND";
const ROOT: &str = "CORRAL_CONDITIONS_CHILD_ROOT";
const TASK: &str = "T-001";

fn a() -> Value {
    json!({"run": "R1", "error": "A"})
}

fn b() -> Value {
    json!({"run": "R1", "error": "B"})
}

fn observe(store: &Store, kind: &str, detail: Value) {
    store.log_event(Some(TASK), kind, detail).unwrap();
}

fn clear(store: &Store, kind: &str) -> ConditionOutcome {
    store.clear_condition(Some(TASK), kind).unwrap().unwrap()
}

fn condition_events(store: &Store, kind: &str) -> Vec<Event> {
    let cleared = format!("{kind}_cleared");
    store.events_of(Some(TASK), &[kind, &cleared]).unwrap()
}

/// Re-executed as the restarted process: A once, B twice, clear, clear.
#[test]
fn child_process() {
    let (Ok(kind), Ok(root)) = (std::env::var(CHILD), std::env::var(ROOT)) else {
        return;
    };
    let store = Store::open(std::path::Path::new(&root)).unwrap();
    let _pm = store.attribute(Actor::Pm);
    observe(&store, &kind, a());
    observe(&store, &kind, b());
    observe(&store, &kind, b());
    assert!(matches!(
        clear(&store, &kind),
        ConditionOutcome::Cleared { .. }
    ));
    assert_eq!(clear(&store, &kind), ConditionOutcome::NotOpen);
}

fn restart_and_finish(root: &std::path::Path, kind: &str) {
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_process", "--test-threads=1"])
        .env(CHILD, kind)
        .env(ROOT, root)
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "{kind}: restarted process failed");
}

fn assert_run(
    run: &ConditionRun,
    opened: &Event,
    count: u64,
    closed: &Event,
    reason: CloseReason,
    actors: (&str, &str),
) {
    assert_eq!(run.opened_seq.0, opened.seq, "{run:?}");
    assert_eq!(run.task, TASK);
    assert_eq!(run.kind, opened.kind);
    assert_eq!(run.count, count, "{run:?}");
    assert_eq!(run.closed_seq.map(|seq| seq.0), Some(closed.seq), "{run:?}");
    assert_eq!(run.close_reason, Some(reason), "{run:?}");
    assert_eq!(run.first_at, opened.ts);
    assert!(run.first_at <= run.last_at, "{run:?}");
    assert_eq!(run.first_actor.as_deref(), Some(actors.0), "{run:?}");
    assert_eq!(run.last_actor.as_deref(), Some(actors.1), "{run:?}");
}

#[test]
fn every_kind_keeps_four_durable_runs_across_changes_clears_and_a_restart() {
    for kind in CONDITION_KINDS {
        // Given A observed 100 times, B once, a clear and A three times.
        let dir = tempfile::tempdir().unwrap();
        {
            let store = Store::init(dir.path()).unwrap();
            let _autopilot = store.attribute(Actor::Autopilot);
            for _ in 0..100 {
                observe(&store, kind, a());
            }
            observe(&store, kind, b());
            assert!(matches!(
                clear(&store, kind),
                ConditionOutcome::Cleared { .. }
            ));
            for _ in 0..3 {
                observe(&store, kind, a());
            }
        }
        // When another process observes A once, B twice, then clears twice.
        restart_and_finish(dir.path(), kind);

        // Then six events record every change and no suppressed repeat or second clear.
        let store = Store::open(dir.path()).unwrap();
        let events = condition_events(&store, kind);
        let cleared = format!("{kind}_cleared");
        let kinds: Vec<_> = events.iter().map(|event| event.kind.as_str()).collect();
        assert_eq!(kinds, [kind, kind, &cleared, kind, kind, &cleared]);
        let details: Vec<_> = events
            .iter()
            .map(|event| event.detail["error"].clone())
            .collect();
        assert_eq!(details[0], "A");
        assert_eq!(details[1], "B");
        assert_eq!(details[3], "A");
        assert_eq!(details[4], "B");
        // And four runs keep their counts, closing events, reasons, times and attribution.
        let runs = store.condition_runs(Some(TASK)).unwrap();
        assert_eq!(runs.len(), 4, "{kind}: {runs:#?}");
        let (autopilot, pm) = ("autopilot", "pm");
        assert_run(
            &runs[0],
            &events[0],
            100,
            &events[1],
            CloseReason::Changed,
            (autopilot, autopilot),
        );
        assert_run(
            &runs[1],
            &events[1],
            1,
            &events[2],
            CloseReason::Cleared,
            (autopilot, autopilot),
        );
        assert_run(
            &runs[2],
            &events[3],
            4,
            &events[4],
            CloseReason::Changed,
            (autopilot, pm),
        );
        assert_run(
            &runs[3],
            &events[4],
            2,
            &events[5],
            CloseReason::Cleared,
            (pm, pm),
        );
        assert!(runs[2].last_at > runs[2].first_at);
        // And each clear carries the run it closed.
        for (clear, run) in [(&events[2], &runs[1]), (&events[5], &runs[3])] {
            assert_eq!(clear.detail["run"], run.opened_seq.0);
            assert_eq!(clear.detail["count"], run.count);
            assert_eq!(clear.detail["first_at"], json!(run.first_at));
            assert_eq!(clear.detail["last_at"], json!(run.last_at));
        }
    }
}

#[test]
fn oscillating_details_record_every_observation_as_a_run_of_one() {
    for kind in ["autopilot_error", "main_movement_notice"] {
        // Given alternating details A, B, A, B.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        for detail in [a(), b(), a(), b()] {
            observe(&store, kind, detail);
        }
        // Then every observation is an event, each closed by the next as changed.
        let events = condition_events(&store, kind);
        assert_eq!(events.len(), 4);
        let runs = store.condition_runs(Some(TASK)).unwrap();
        assert_eq!(runs.len(), 4);
        for (i, run) in runs.iter().enumerate() {
            assert_eq!(run.count, 1);
            assert_eq!(run.opened_seq.0, events[i].seq);
            match events.get(i + 1) {
                Some(next) => {
                    assert_eq!(run.closed_seq.map(|seq| seq.0), Some(next.seq));
                    assert_eq!(run.close_reason, Some(CloseReason::Changed));
                }
                None => assert_eq!(run.closed_seq, None),
            }
        }
    }
}

/// One update through the full (`update`) or head (`update_head`) facade whose callback logs
/// `kind` with `detail`, then refuses when `refuse` is set.
fn update_logging(
    store: &Store,
    id: &str,
    head: bool,
    kind: &str,
    detail: Value,
    refuse: bool,
) -> anyhow::Result<()> {
    let log = |rounds: &mut u32| {
        *rounds += 1;
        store.log_event(Some(id), kind, detail.clone())?;
        anyhow::ensure!(!refuse, "refused");
        Ok(())
    };
    if head {
        store.update_head(id, |t| log(&mut t.rounds)).map(drop)
    } else {
        store.update(id, |t| log(&mut t.rounds)).map(drop)
    }
}

#[test]
fn conditions_inside_task_updates_count_on_one_run_and_vanish_with_a_refusal() {
    for head in [false, true] {
        // Given a task.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let task = store.create("Queued", None, Status::Ready, "").unwrap();
        let queued = |store: &Store| store.events_of(Some(&task.id), &["queued"]).unwrap();
        // When a refused callback observes a condition.
        let tail = store.event_tail().unwrap();
        assert!(update_logging(&store, &task.id, head, "queued", a(), true).is_err());
        // Then neither the task nor any event or run changed.
        assert_eq!(store.get(&task.id).unwrap().rounds, task.rounds);
        assert_eq!(store.event_tail().unwrap(), tail);
        assert!(store.condition_runs(None).unwrap().is_empty());

        // When three successful updates observe the identical detail.
        for _ in 0..3 {
            update_logging(&store, &task.id, head, "queued", a(), false).unwrap();
        }
        // Then one event opened one run that counts all three, and each task write committed.
        assert_eq!(store.get(&task.id).unwrap().rounds, task.rounds + 3);
        let events = queued(&store);
        assert_eq!(events.len(), 1, "head={head}");
        let runs = store.condition_runs(Some(&task.id)).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!((runs[0].opened_seq.0, runs[0].count), (events[0].seq, 3));

        // When a refused callback repeats it, then a successful one changes the detail.
        assert!(update_logging(&store, &task.id, head, "queued", a(), true).is_err());
        update_logging(&store, &task.id, head, "queued", b(), false).unwrap();
        // Then the refusal counted nothing and the change closed the first run.
        let runs = store.condition_runs(Some(&task.id)).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].count, 3);
        assert_eq!(runs[0].close_reason, Some(CloseReason::Changed));
        assert_eq!(queued(&store).len(), 2);

        // When a callback clears the condition inside an update that refuses, then one that commits.
        let clear = |refuse: bool| {
            store.update(&task.id, |_| {
                assert_eq!(store.clear_condition(Some(&task.id), "queued")?, None);
                anyhow::ensure!(!refuse, "refused");
                Ok(())
            })
        };
        assert!(clear(true).is_err());
        assert!(
            store.condition_runs(Some(&task.id)).unwrap()[1]
                .closed_seq
                .is_none()
        );
        clear(false).unwrap();
        // Then only the committed update cleared it.
        let cleared = store
            .events_of(Some(&task.id), &["queued_cleared"])
            .unwrap();
        assert_eq!(cleared.len(), 1);
        assert_eq!(cleared[0].detail["count"], 1);
    }
}

#[test]
fn leaving_ready_clears_queued_and_dispatch_failed_once() {
    for head in [false, true] {
        // Given a Ready task with open queued and dispatch_failed runs.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let task = store.create("Ready", None, Status::Ready, "").unwrap();
        for _ in 0..3 {
            store
                .log_event(Some(&task.id), "queued", json!({"reason": "slots full"}))
                .unwrap();
        }
        store
            .log_event(Some(&task.id), "dispatch_failed", json!({"error": "boom"}))
            .unwrap();
        // When an unrelated write keeps it Ready, then a write moves it to Running.
        store
            .update(&task.id, |t| {
                t.rounds += 1;
                Ok(())
            })
            .unwrap();
        assert!(
            store
                .events_of(Some(&task.id), &["queued_cleared"])
                .unwrap()
                .is_empty()
        );
        if head {
            store
                .update_head(&task.id, |t| {
                    t.status = Status::Running;
                    Ok(())
                })
                .unwrap();
        } else {
            store
                .update(&task.id, |t| {
                    t.status = Status::Running;
                    Ok(())
                })
                .unwrap();
        }
        store
            .update(&task.id, |t| {
                t.status = Status::Review;
                Ok(())
            })
            .unwrap();
        // Then each kind is cleared once with its run's count, and no run stays open.
        let queued = store
            .events_of(Some(&task.id), &["queued_cleared"])
            .unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].detail["count"], 3);
        let failed = store
            .events_of(Some(&task.id), &["dispatch_failed_cleared"])
            .unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].detail["count"], 1);
        let runs = store.condition_runs(Some(&task.id)).unwrap();
        assert!(
            runs.iter()
                .all(|run| run.close_reason == Some(CloseReason::Cleared))
        );
    }
}

#[test]
fn a_kind_whose_resolution_is_never_observed_keeps_its_open_run_durable() {
    // Given an observed landing_stale run that nothing resolves.
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::init(dir.path()).unwrap();
        observe(&store, "landing_stale", a());
        observe(&store, "landing_stale", a());
    }
    // When the store is reopened.
    let store = Store::open(dir.path()).unwrap();
    // Then the open run and its count survive.
    let runs = store.condition_runs(Some(TASK)).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].count, 2);
    assert_eq!(runs[0].closed_seq, None);
}

#[test]
fn verdict_notes_of_sixteen_kib_are_a_blob_ref_that_resolves_to_the_text() {
    // Given a task and notes just below and at the slim threshold.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let task = store.create("Review", None, Status::Review, "").unwrap();
    let short = "n".repeat(corral_core::SLIM_TEXT_BYTES - 1);
    let long = "x".repeat(20 * 1024);
    // When both are recorded as verdict events inside task updates.
    for notes in [&short, &long] {
        store
            .update(&task.id, |t| {
                let value = store.event_text(notes)?;
                store.log_event(
                    Some(&t.id),
                    "verdict",
                    json!({"verdict": "approve", "notes": value}),
                )
            })
            .unwrap();
    }
    // Then the short text is inline and the long one a BlobRef that reads back exactly.
    let events = store.events_of(Some(&task.id), &["verdict"]).unwrap();
    assert_eq!(events[0].detail["notes"], short.as_str());
    let reference = events[1].detail["notes"].as_str().unwrap();
    let blob: corral_core::blob_ref::BlobRef = reference.parse().unwrap();
    assert_eq!(
        blob.hash(),
        &corral_core::blob_ref::BlobHash::of(long.as_bytes())
    );
    assert_eq!(
        store
            .resolve_text(&events[1].detail["notes"])
            .unwrap()
            .as_deref(),
        Some(long.as_str())
    );
}
