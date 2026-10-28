//! The usage refresh's guard on its real path (plan T-253 section 4, "CAS guards"): it compares
//! the assembled current run exactly, as `db316644` compared the whole run, so every write
//! between the refresh's read and its guard is decided as `db316644` decides it: a genuine
//! change (here, to an earlier review child's usage) is refused and the next tick refreshes,
//! and A->B->A is accepted.
use corral_core::{Run, Status, Store, Usage, UsageEntry};
use serde_json::json;

use super::*;

fn entry(input: u64) -> UsageEntry {
    serde_json::from_value(json!({
        "at": "2026-10-19T00:10:00Z", "latency_ms": 10, "model": "m", "input": input,
        "output": 5, "cache_read": 0, "cache_write": 0, "reported_usd": null,
        "estimated_usd": null, "unpriced_tokens": 0
    }))
    .unwrap()
}

fn fixture() -> (tempfile::TempDir, Store, String) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Usage", None, Status::Running, "").unwrap();
    let child = |n: u32| {
        json!({
            "id": format!("R1-review{n}"), "agent": "reviewer", "harness": "zz",
            "worktree": tmp.path(), "branch": "main", "base": "main", "base_sha": "fixture",
            "started": "2026-10-19T00:05:00Z", "ended": "2026-10-19T00:20:00Z",
            "outcome": "done", "usage": Usage::from_entries(vec![entry(10)])
        })
    };
    // The run's unsupported harness gives the refresh one change to write: its usage note.
    let run: Run = serde_json::from_value(json!({
        "id": "R1", "agent": "worker", "harness": "zz", "worktree": tmp.path(),
        "branch": "main", "base": "main", "base_sha": "fixture",
        "started": "2026-10-19T00:00:00Z", "review_runs": [child(1), child(2)]
    }))
    .unwrap();
    store
        .update(&task.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    // Settle the children's prices, so only the note is left to refresh.
    store
        .update(&task.id, |t| {
            for child in &mut t.runs[0].review_runs {
                if let Some(usage) = &mut child.usage {
                    usage.reprice(&store.config, child.model.as_deref());
                }
            }
            Ok(())
        })
        .unwrap();
    (tmp, store, task.id)
}

fn set_earlier_child(store: &Store, id: &str, input: u64) {
    store
        .update_head(id, |head| {
            head.edit_review_child("R1", "R1-review1", |child| {
                let mut usage = Usage::from_entries(vec![entry(input)]);
                usage.reprice(&store.config, child.model.as_deref());
                child.usage = Some(usage);
                Ok(())
            })
        })
        .unwrap();
}

fn interleave(store: &Store, id: &str, kind: &str) {
    match kind {
        "none" => {}
        "identical" => set_earlier_child(store, id, 10),
        "genuine" => set_earlier_child(store, id, 99),
        "aba" => {
            set_earlier_child(store, id, 99);
            set_earlier_child(store, id, 10);
        }
        other => panic!("unknown interleaving {other}"),
    }
}

/// What the refresh and its guard decide: the run's usage and note, and each child's usage.
#[derive(Debug, Clone, PartialEq)]
struct Refreshed {
    usage_note: Option<String>,
    usage: Option<Usage>,
    children: Vec<Option<Usage>>,
}

fn run(store: &Store, id: &str) -> Refreshed {
    let run: Run = store.get(id).unwrap().runs[0].clone();
    Refreshed {
        usage_note: run.usage_note,
        usage: run.usage,
        children: run.review_runs.into_iter().map(|c| c.usage).collect(),
    }
}

fn passes(kind: &str) -> (Refreshed, Refreshed) {
    let (_tmp, store, id) = fixture();
    // The refresh reads its head, then a writer commits between that read and the guard.
    let head = store.head(&id).unwrap();
    interleave(&store, &id, kind);
    refresh_current(&store, &head).unwrap();
    let first = run(&store, &id);
    refresh_current(&store, &store.head(&id).unwrap()).unwrap();
    (first, run(&store, &id))
}

#[test]
fn the_usage_guard_compares_the_whole_current_run_exactly() {
    let note = Some("unsupported harness zz".to_owned());
    let (none_first, none_second) = passes("none");
    assert_eq!(none_first.usage_note, note);
    assert_eq!(none_second, none_first);

    // Equal stored bytes and A->B->A both leave the whole run equal: accepted, as db316644.
    assert_eq!(
        passes("identical"),
        (none_first.clone(), none_second.clone())
    );
    assert_eq!(passes("aba"), (none_first, none_second));

    // A genuine change to an earlier review child's usage is refused; the next tick refreshes
    // on top of it.
    let (genuine_first, genuine_second) = passes("genuine");
    assert_eq!(genuine_first.usage_note, None);
    assert_eq!(genuine_second.usage_note, note);
    let earlier = |run: &Refreshed| run.children[0].as_ref().map(|u| u.input);
    assert_eq!(earlier(&genuine_first), Some(99));
    assert_eq!(earlier(&genuine_second), Some(99));
}
