//! The refresh's verdict selection for an unended reviewer (T-330 review R1-3, R2-2): it takes
//! the earliest verdict time at or after the review's start over every verdict event of the
//! run, as the full selection does, whatever order the events' times have; and once a child has
//! been observed, 10x earlier verdict events in the run cost nothing more.
use corral_core::Event;
use corral_core::work::{self, Counter, HotScope};
use serde_json::json;

use super::*;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_second(1_800_000_000 + secs).unwrap()
}

fn event(ts: Timestamp, kind: &str, run: &str) -> Event {
    Event {
        seq: 0,
        ts,
        task: Some("T-001".into()),
        kind: kind.into(),
        actor: None,
        via: None,
        detail: json!({"run": run}),
    }
}

fn append(store: &Store, events: &[Event]) {
    let log: String = events
        .iter()
        .map(|e| serde_json::to_string(e).unwrap() + "\n")
        .collect();
    store.fixture_append_events(&log).unwrap();
}

/// `earlier` verdict-kind events of run R1 before the review started at `at(1000)`, other
/// runs' verdicts and other kinds between them, then verdicts at `after` offsets from the start
/// in that sequence order (negative offsets precede the start).
fn store(root: &std::path::Path, earlier: usize, after: &[i64]) -> Store {
    let store = Store::init(root).unwrap();
    store
        .create("Reviewed", None, corral_core::Status::Review, "")
        .unwrap();
    let mut events = Vec::new();
    for k in 0..earlier {
        let ts = at(i64::try_from(k).unwrap());
        events.push(event(ts, VERDICT_KINDS[k % VERDICT_KINDS.len()], "R1"));
        events.push(event(ts, "verdict", "R0"));
        events.push(event(ts, "comment", "R1"));
    }
    events.push(event(at(1000), "review_started", "R1"));
    events.extend(
        after
            .iter()
            .map(|offset| event(at(1000 + offset), "verdict", "R1")),
    );
    append(&store, &events);
    store
}

/// The full selection (`refresh_runs`): every verdict event of the run, the earliest time at or
/// after `from`.
fn selected(store: &Store, from: Timestamp) -> Option<Timestamp> {
    store
        .events_of_run(Some("T-001"), &VERDICT_KINDS, Some("R1"), 0, u32::MAX)
        .unwrap()
        .into_iter()
        .filter(|event| event.ts >= from)
        .map(|event| event.ts)
        .min()
}

#[test]
fn the_first_verdict_is_the_full_selections_whatever_the_event_order() {
    let orders: [&[i64]; 7] = [
        &[],
        &[0],
        &[5],
        &[5, 9, 12],
        &[60, -60],
        &[11, -1, 3],
        &[-5, 30, 7],
    ];
    for earlier in [0, 1, 3, 17] {
        for after in orders {
            for from in [at(0), at(999), at(1000), at(1006)] {
                let tmp = tempfile::tempdir().unwrap();
                let store = store(tmp.path(), earlier, after);
                assert_eq!(
                    first_verdict_from(&store, "T-001", "R1", "R1-review1", from).unwrap(),
                    selected(&store, from),
                    "{earlier} earlier, {after:?} after, from {from}"
                );
                // Later verdicts, out of order too, are folded in by the kept position.
                append(
                    &store,
                    &[
                        event(at(1003), "verdict", "R1"),
                        event(at(1001), "approved", "R1"),
                    ],
                );
                assert_eq!(
                    first_verdict_from(&store, "T-001", "R1", "R1-review1", from).unwrap(),
                    selected(&store, from),
                    "{earlier} earlier, {after:?} after, from {from}, then two more"
                );
            }
        }
    }
}

#[test]
fn an_out_of_order_imported_verdict_ends_the_child_through_the_real_refresh() {
    // Given an unended reviewer that started at 00:10 and verdicts imported in the order 00:11,
    // then 00:09.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("Reviewed", None, corral_core::Status::Review, "")
        .unwrap();
    let ts = |s: &str| s.parse::<Timestamp>().unwrap();
    let run: Run = serde_json::from_value(json!({
        "id": "R1", "agent": "fixture", "worktree": tmp.path(), "branch": "main", "base": "main",
        "base_sha": "fixture", "started": "2026-10-20T00:00:00Z",
        "review_runs": [{"id": "R1-review1", "agent": "fixture", "worktree": tmp.path(),
                         "branch": "main", "base": "main", "base_sha": "fixture",
                         "started": "2026-10-20T00:10:00Z"}]
    }))
    .unwrap();
    store
        .update(&task.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    append(
        &store,
        &[
            event(ts("2026-10-20T00:11:00Z"), "verdict", "R1"),
            event(ts("2026-10-20T00:09:00Z"), "verdict", "R1"),
        ],
    );
    // When the board's usage refresh runs.
    refresh_active_usage(&store);
    // Then the child ends at the earliest verdict at or after its start, as the full selection.
    let child = store.get(&task.id).unwrap().runs[0].review_runs[0].clone();
    assert_eq!(child.ended, Some(ts("2026-10-20T00:11:00Z")));
}

fn measured(store: &Store) -> Vec<(&'static str, u64)> {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    assert_eq!(
        first_verdict_from(store, "T-001", "R1", "R1-review1", at(1000)).unwrap(),
        None
    );
    let delta = work::snapshot().since(&before);
    assert_eq!(
        delta.get(Counter::EventDecodes),
        0,
        "no verdict is read again"
    );
    Counter::ALL
        .iter()
        .map(|c| (c.name(), delta.cells(*c).unattributed))
        .collect()
}

#[test]
fn an_unended_reviewer_does_not_reread_the_runs_earlier_verdicts() {
    let tmp = tempfile::tempdir().unwrap();
    let base = store(&tmp.path().join("small"), 4, &[]);
    let grown = store(&tmp.path().join("large"), 40, &[]);
    for store in [&base, &grown] {
        // The first refresh of the child observes the run's verdicts once.
        first_verdict_from(store, "T-001", "R1", "R1-review1", at(1000)).unwrap();
    }
    assert_eq!(measured(&base), measured(&grown));
}
