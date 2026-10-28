//! A sync that is not yet due for its scheduled digest reads one record (T-330 review R1-3):
//! with 1x and 10x manual digests after the last scheduled one, and after recovering a store
//! that predates the record, it decodes no event and does the same work, and the recorded
//! snapshot is the one the full scan finds.
use corral_core::Event;
use corral_core::work::{self, Counter, HotScope};
use serde_json::json;

use super::*;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_second(1_800_000_000 + secs).unwrap()
}

fn ctx(root: &std::path::Path, manual: usize) -> Ctx {
    let mut store = corral_core::Store::init(root).unwrap();
    store.config.digest.every = Some("1d".into());
    let digest = |ts: Timestamp, scheduled: bool| Event {
        seq: 0,
        ts,
        task: None,
        kind: "digest".into(),
        actor: None,
        via: None,
        detail: json!({"since": ts, "snapshot_at": ts, "scheduled": scheduled}),
    };
    let mut events = vec![digest(at(0), true)];
    events.extend((1..=manual).map(|k| digest(at(i64::try_from(k).unwrap()), false)));
    let log: String = events
        .iter()
        .map(|e| serde_json::to_string(e).unwrap() + "\n")
        .collect();
    store.fixture_append_events(&log).unwrap();
    Ctx::from_store(store)
}

fn not_due(ctx: &Ctx) -> Vec<(&'static str, u64)> {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    assert!(scheduled_digest_at(ctx, at(3600)).unwrap().is_none());
    let delta = work::snapshot().since(&before);
    assert_eq!(delta.get(Counter::EventDecodes), 0);
    Counter::ALL
        .iter()
        .map(|c| (c.name(), delta.cells(*c).unattributed))
        .collect()
}

#[test]
fn a_sync_before_the_next_scheduled_digest_does_not_read_the_manual_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let base = ctx(&tmp.path().join("small"), 2);
    let grown = ctx(&tmp.path().join("large"), 20);
    for ctx in [&base, &grown] {
        let scanned = ctx
            .store
            .last_event_where(None, &["digest"], None, |e| e.detail["scheduled"] == true)
            .unwrap()
            .map(|e| e.ts);
        // The first sync recovers the record outside the measured pass, from the full scan.
        not_due(ctx);
        assert_eq!(last_scheduled(ctx).unwrap(), scanned);
    }
    assert_eq!(not_due(&base), not_due(&grown));
}
