//! The refresh's verdict selection for an unended reviewer (T-330 review R1-3): it decides as
//! the earlier sequence-order scan from the start of the run, and with 10x earlier verdict
//! events in the refreshed run it decodes the same events.
use corral_core::Event;
use corral_core::work::{self, Counter, HotScope};
use serde_json::json;

use super::*;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_second(1_800_000_000 + secs).unwrap()
}

/// `earlier` verdict-kind events of run R1 before the review started at `at(1000)`, other
/// runs' verdicts and other kinds between them, then verdicts at `after` offsets.
fn store(root: &std::path::Path, earlier: usize, after: &[i64]) -> Store {
    let store = Store::init(root).unwrap();
    store
        .create("Reviewed", None, corral_core::Status::Review, "")
        .unwrap();
    let event = |ts: Timestamp, kind: &str, run: &str| Event {
        seq: 0,
        ts,
        task: Some("T-001".into()),
        kind: kind.into(),
        actor: None,
        via: None,
        detail: json!({"run": run}),
    };
    let mut events = Vec::new();
    for k in 0..earlier {
        let ts = at(i64::try_from(k).unwrap());
        events.push(event(ts, VERDICT_KINDS[k % VERDICT_KINDS.len()], "R1"));
        events.push(event(ts, "verdict", "R0"));
        events.push(event(ts, "comment", "R1"));
    }
    events.push(event(at(1000), "review_started", "R1"));
    for offset in after {
        events.push(event(at(1000 + offset), "verdict", "R1"));
    }
    let log: String = events
        .iter()
        .map(|e| serde_json::to_string(e).unwrap() + "\n")
        .collect();
    store.fixture_append_events(&log).unwrap();
    store
}

/// The selection before review R1-3: every verdict of the run from the start, in sequence
/// order, the first at or after `from`.
fn scanned(store: &Store, from: Timestamp) -> Option<Timestamp> {
    store
        .events_of_run(Some("T-001"), &VERDICT_KINDS, Some("R1"), 0, u32::MAX)
        .unwrap()
        .into_iter()
        .find(|event| event.ts >= from)
        .map(|event| event.ts)
}

#[test]
fn the_first_verdict_is_selected_as_the_scan_from_the_run_start_selects_it() {
    for earlier in [0, 1, 3, 17] {
        for after in [&[][..], &[0], &[5], &[5, 9, 12]] {
            let tmp = tempfile::tempdir().unwrap();
            let store = store(tmp.path(), earlier, after);
            for from in [at(0), at(999), at(1000), at(1006)] {
                assert_eq!(
                    first_verdict_from(&store, "T-001", "R1", from).unwrap(),
                    scanned(&store, from),
                    "{earlier} earlier, {after:?} after, from {from}"
                );
            }
        }
    }
}

fn measured(store: &Store) -> Vec<(&'static str, u64)> {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    assert_eq!(
        first_verdict_from(store, "T-001", "R1", at(1000)).unwrap(),
        None
    );
    let delta = work::snapshot().since(&before);
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
    assert_eq!(measured(&base), measured(&grown));
}
