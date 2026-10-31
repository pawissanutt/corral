//! The reviewer launch guard (plan T-253 section 4, "CAS guards", `reviewer.rs:384-388`) through
//! the real launch: between the launch's read and its guarded write, a barrier applies one
//! interleaving of child inserts and edits. The guard must decide exactly as `db316644`'s
//! predicate over the full task does (the child count and the parent's review record), with the
//! refusal's state, events and records, the next pass, and an earlier child's usage as expected.
use super::*;
use corral_core::{ReviewRecord, Task, UsageEntry, Verdict};
use serde_json::json;

const POINT: &str = "review_child_observed";
const REFUSAL: &str = "review changed before preparing accounting child";

#[derive(Debug, Clone, Copy)]
enum Interleaving {
    Nothing,
    /// Another accounting child is inserted: the count changes.
    PushChild,
    /// The parent's review record is set: the record changes.
    SetRecord,
    /// The record is set and cleared again (A->B->A): equal to `db316644`'s read.
    SetRecordBack,
    /// An earlier child's summary and usage change: outside the guard's read set.
    EditEarlierChild,
    /// An earlier child is edited and edited back.
    EditEarlierChildBack,
}

const ALL: [Interleaving; 6] = [
    Interleaving::Nothing,
    Interleaving::PushChild,
    Interleaving::SetRecord,
    Interleaving::SetRecordBack,
    Interleaving::EditEarlierChild,
    Interleaving::EditEarlierChildBack,
];

fn usage(tokens: u64) -> corral_core::Usage {
    corral_core::Usage::from_entries(vec![UsageEntry {
        at: Some(fixture::now()),
        latency_ms: Some(1),
        model: Some("earlier".into()),
        input: tokens,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        reported_usd: None,
        estimated_usd: None,
        price_source: None,
        unpriced_tokens: 0,
    }])
}

/// A Review task with a passed gate and one earlier, finished review child with usage.
fn setup() -> (Fx, String) {
    let (fx, id) = Path::ReviewerPane.setup();
    let review = crate::review_run(&fx.ctx, &id, Some("rev")).unwrap();
    let token = crate::review_callback_token(&review).unwrap();
    crate::verdict_bound(&fx.ctx, &id, Verdict::Changes, "fix it", Some(&token)).unwrap();
    fx.ctx
        .store
        .update(&id, |t| {
            let child = t.current_run_mut().unwrap().review_runs.last_mut().unwrap();
            child.usage = Some(usage(7));
            Ok(())
        })
        .unwrap();
    (fx, id)
}

fn event_kinds(fx: &Fx, id: &str) -> Vec<String> {
    let events = fx.ctx.store.events().unwrap();
    let mine = events.iter().filter(|e| e.task.as_deref() == Some(id));
    mine.map(|e| e.kind.clone()).collect()
}

fn edit(store: &corral_core::Store, id: &str, change: impl FnOnce(&mut Task)) {
    store
        .update(id, |t| {
            change(t);
            Ok(())
        })
        .unwrap();
}

fn apply(store: &corral_core::Store, id: &str, interleaving: Interleaving) {
    let record = |t: &Task| -> ReviewRecord {
        let run = t.current_run().unwrap();
        let mut record = run.review_runs[0].review.clone().unwrap();
        record.notes = Some("an interleaved record".into());
        record
    };
    match interleaving {
        Interleaving::Nothing => {}
        Interleaving::PushChild => edit(store, id, |t| {
            let run = t.current_run_mut().unwrap();
            let mut child = run.review_runs[0].clone();
            child.id = format!("{}-interleaved", run.id);
            run.review_runs.push(child);
        }),
        Interleaving::SetRecord | Interleaving::SetRecordBack => {
            let original = store.get(id).unwrap().current_run().unwrap().review.clone();
            edit(store, id, |t| {
                let record = record(t);
                t.current_run_mut().unwrap().review = Some(record);
            });
            if matches!(interleaving, Interleaving::SetRecordBack) {
                edit(store, id, |t| {
                    t.current_run_mut().unwrap().review = original
                });
            }
        }
        Interleaving::EditEarlierChild | Interleaving::EditEarlierChildBack => {
            let original = store.get(id).unwrap().current_run().unwrap().review_runs[0].clone();
            edit(store, id, |t| {
                let child = &mut t.current_run_mut().unwrap().review_runs[0];
                child.usage = Some(usage(11));
                child.summary = Some("interleaved".into());
            });
            if matches!(interleaving, Interleaving::EditEarlierChildBack) {
                edit(store, id, |t| {
                    t.current_run_mut().unwrap().review_runs[0] = original;
                });
            }
        }
    }
}

/// `db316644`'s guard: the full task's child count and the parent's review record.
fn db316644_refuses(before: &Task, after: &Task) -> bool {
    let (b, a) = (before.current_run().unwrap(), after.current_run().unwrap());
    b.review_runs.len() != a.review_runs.len() || b.review != a.review
}

#[derive(Debug, PartialEq)]
struct Outcome {
    refused: bool,
    error: Option<String>,
    /// Children the launch added beyond the interleaving's state.
    pushed: usize,
    events: Vec<String>,
    pane_runs: usize,
    reservation: Option<corral_core::ReviewWork>,
    /// The earlier child's usage after the pass, against the interleaving's.
    earlier_usage_kept: bool,
    next_pass_launched: bool,
}

fn run(interleaving: Interleaving) -> (Outcome, bool) {
    let (mut fx, id) = setup();
    let before = fx.ctx.store.get(&id).unwrap();
    let events = event_kinds(&fx, &id);
    let pane_runs = fx.records("submissions.jsonl").len();
    let plain = fx.ctx.store.clone();
    let edits = plain.clone();
    let observed = Arc::new(Mutex::new(None::<(Task, Vec<String>)>));
    let seen = observed.clone();
    let fired = Mutex::new(false);
    let task = id.clone();
    fx.ctx.store = fx.ctx.store.with_submission_barrier(Arc::new(move |at| {
        if at != POINT || std::mem::replace(&mut *fired.lock().unwrap(), true) {
            return;
        }
        apply(&edits, &task, interleaving);
        let at_guard = edits
            .events()
            .unwrap()
            .iter()
            .filter(|event| event.task.as_deref() == Some(&task))
            .map(|event| event.kind.clone())
            .collect();
        *seen.lock().unwrap() = Some((edits.get(&task).unwrap(), at_guard));
    }));
    let launched = crate::review_run(&fx.ctx, &id, Some("rev"));
    let (between, at_guard) = observed.lock().unwrap().take().expect("the barrier fired");
    // Claiming precedes this CAS read/write boundary. Validate it explicitly instead of
    // filtering audit events: every event after the actual guard is compared exactly.
    assert_eq!(&at_guard[..events.len()], events.as_slice());
    assert_eq!(&at_guard[events.len()..], &["review_claimed".to_string()]);
    let expected = db316644_refuses(&before, &between);
    fx.ctx.store = plain;
    let after = fx.ctx.store.get(&id).unwrap();
    let earlier = |t: &Task| t.current_run().unwrap().review_runs[0].usage.clone();
    let error = launched.as_ref().err().map(|e| format!("{e:#}"));
    let refused = error.as_deref().is_some_and(|e| e.contains(REFUSAL));
    let mut outcome = Outcome {
        refused,
        error: error.filter(|e| !e.contains(REFUSAL)),
        pushed: after.current_run().unwrap().review_runs.len()
            - between.current_run().unwrap().review_runs.len(),
        events: event_kinds(&fx, &id)
            .into_iter()
            .skip(at_guard.len())
            .collect(),
        pane_runs: fx.records("submissions.jsonl").len() - pane_runs,
        reservation: after.review_reservation,
        earlier_usage_kept: earlier(&after) == earlier(&between),
        next_pass_launched: false,
    };
    if !refused {
        let audits = fx.ctx.store.events().unwrap();
        let audit = audits
            .iter()
            .filter(|event| event.task.as_deref() == Some(&id))
            .nth(at_guard.len())
            .unwrap();
        assert_eq!(audit.kind, "role_session");
        assert_eq!(audit.actor, Some(corral_core::Actor::Reviewer));
        let child = after.current_run().unwrap().review_runs.last().unwrap();
        assert_eq!(audit.detail["run"], json!(child.id));
        assert_eq!(
            audit.detail["session"],
            serde_json::to_value(child.session.as_ref().unwrap()).unwrap()
        );
    }
    if refused {
        outcome.next_pass_launched = crate::review_run(&fx.ctx, &id, Some("rev")).is_ok();
    }
    (outcome, expected)
}

#[test]
fn reviewer_launch_guard_decides_as_db316644_on_every_interleaving() {
    let mut failures = Vec::new();
    for interleaving in ALL {
        let (outcome, expected) = run(interleaving);
        if outcome.refused != expected {
            failures.push(format!(
                "{interleaving:?}: refused {} where db316644 refuses {expected}: {outcome:?}",
                outcome.refused
            ));
            continue;
        }
        let want = if expected {
            Outcome {
                refused: true,
                error: None,
                pushed: 0,
                events: Vec::new(),
                pane_runs: 0,
                reservation: None,
                earlier_usage_kept: true,
                next_pass_launched: true,
            }
        } else {
            Outcome {
                refused: false,
                error: None,
                pushed: 1,
                events: vec!["role_session".into(), "review_started".into()],
                pane_runs: 1,
                reservation: None,
                earlier_usage_kept: true,
                next_pass_launched: false,
            }
        };
        let got = outcome;
        if got != want {
            failures.push(format!("{interleaving:?}: {got:?}, expected {want:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
