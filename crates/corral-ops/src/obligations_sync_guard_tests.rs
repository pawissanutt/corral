//! The obligations tick's guard on its real path (plan T-253 section 4, "CAS guards"):
//! `obligations_rev` stands in for `db316644`'s whole-obligations comparison. A write between
//! the tick's read and its guard is decided as `db316644` decides it, except A->B->A, which is
//! refused; after that refusal the next tick behaves exactly as after a genuine change.
use corral_core::{DecisionRecord, Event, Run, RunObligations, Status, Store, WaitRecord};
use jiff::Timestamp;
use serde_json::json;

use super::*;

const NOW: &str = "2026-10-20T00:00:00Z";

fn fixture() -> (tempfile::TempDir, Ctx, String) {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx {
        store: Store::init(tmp.path()).unwrap(),
        herdr: corral_herdr::Herdr::with_bin(tmp.path().join("no-herdr"), None),
        resource_probe: crate::resources::FixedResourceProbe::arc(),
    };
    let task = ctx
        .store
        .create("Waits", None, Status::Running, "")
        .unwrap();
    let receipt = |id: &str| {
        json!({"version": 1, "id": format!("rc-{id}"), "run": "R1", "outcome": "answered",
               "delivery": "acknowledged", "observed_at": "2026-10-19T00:00:00Z", "answer": "yes"})
    };
    let run: Run = serde_json::from_value(json!({
        "id": "R1", "agent": "claude", "worktree": tmp.path(), "branch": "main", "base": "main",
        "base_sha": "fixture", "started": "2026-10-19T00:00:00Z",
        "obligations": {
            "waits": [
                // Due: the tick expires it, the change both guards must apply exactly once.
                {"request": {"version": 1, "id": "W1", "run": "R1", "condition": "ci green",
                             "owner": "worker", "source": "worker",
                             "deadline": "2026-10-19T12:00:00Z",
                             "handle": {"kind": "channel", "channel_id": "c1", "capability": "wait_v1"}},
                 "registered_at": "2026-10-19T00:00:00Z",
                 "observation": {"at": "2026-10-19T00:00:00Z", "state": "pending", "detail": "registered"}},
                // Final: only the interleaved writer edits it.
                {"request": {"version": 1, "id": "W2", "run": "R1", "condition": "review",
                             "owner": "worker", "source": "worker", "deadline": null,
                             "handle": {"kind": "channel", "channel_id": "c2", "capability": "wait_v1"}},
                 "registered_at": "2026-10-19T00:00:00Z",
                 "observation": {"at": "2026-10-19T00:00:00Z", "state": "ready",
                                 "detail": "receipt rc-W2: Answered; delivery Acknowledged"},
                 "receipt": receipt("W2")}
            ],
            "decisions": []
        }
    }))
    .unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    (tmp, ctx, task.id)
}

fn set_w2(ctx: &Ctx, id: &str, condition: &str) {
    ctx.store
        .update_head(id, |head| {
            head.edit_wait("R1", "W2", |wait| {
                wait.request.condition = condition.into();
                Ok(())
            })
        })
        .unwrap();
}

fn interleave(ctx: &Ctx, id: &str, kind: &str) {
    match kind {
        "none" => {}
        // Rewrites the stored document unchanged: never a conflict.
        "identical" => set_w2(ctx, id, "review"),
        "genuine" => set_w2(ctx, id, "B"),
        "aba" => {
            set_w2(ctx, id, "B");
            set_w2(ctx, id, "review");
        }
        other => panic!("unknown interleaving {other}"),
    }
}

fn obligations(ctx: &Ctx, id: &str) -> RunObligations {
    ctx.store.get(id).unwrap().runs[0].obligations.clone()
}

fn transitions(ctx: &Ctx, id: &str) -> Vec<Event> {
    ctx.store.events_of(Some(id), &["wait_transition"]).unwrap()
}

fn w(o: &RunObligations, id: &str) -> WaitRecord {
    o.waits.iter().find(|w| w.request.id == id).unwrap().clone()
}

/// One interleaving: the first tick's disposition and state, then the next tick's.
#[derive(Debug, PartialEq)]
struct Pass {
    applied: bool,
    w1: WaitRecord,
    transitions: Vec<serde_json::Value>,
    decisions: Vec<DecisionRecord>,
}

fn pass(ctx: &Ctx, id: &str, applied: bool) -> Pass {
    let o = obligations(ctx, id);
    Pass {
        applied,
        w1: w(&o, "W1"),
        transitions: transitions(ctx, id)
            .iter()
            .map(|e| e.detail.clone())
            .collect(),
        decisions: o.decisions,
    }
}

fn run(kind: &str) -> (Pass, Pass, WaitRecord) {
    let (_tmp, ctx, id) = fixture();
    let now: Timestamp = NOW.parse().unwrap();
    // The tick reads its head, then a writer commits between that read and the guard.
    let head = ctx.store.head(&id).unwrap();
    interleave(&ctx, &id, kind);
    let first = reconcile_head(&ctx, &head, now, None).unwrap();
    let first = pass(&ctx, &id, first.is_some());
    let second = reconcile_one(&ctx, &id, now, None).unwrap();
    let second = pass(&ctx, &id, second.is_some());
    (first, second, w(&obligations(&ctx, &id), "W2"))
}

#[test]
fn obligations_rev_decides_as_the_whole_comparison_except_aba() {
    let (none_first, none_second, _) = run("none");
    assert!(none_first.applied && !none_second.applied);
    assert_eq!(none_first.transitions.len(), 1);
    assert_eq!(
        none_first.w1.observation.state,
        corral_core::WaitState::Expired
    );

    // A write leaving the stored bytes equal is accepted, exactly as with no write.
    let (identical_first, identical_second, _) = run("identical");
    assert_eq!(identical_first, none_first);
    assert_eq!(identical_second, none_second);

    // A genuine change is refused by both guards: nothing written, no event; the next tick
    // applies the change once.
    let (genuine_first, genuine_second, genuine_w2) = run("genuine");
    assert!(!genuine_first.applied);
    assert!(genuine_first.transitions.is_empty());
    assert_eq!(
        genuine_first.w1.observation.state,
        corral_core::WaitState::Pending
    );
    assert!(genuine_second.applied);
    assert_eq!(genuine_second.w1, none_first.w1);
    assert_eq!(genuine_second.transitions, none_first.transitions);
    assert_eq!(genuine_w2.request.condition, "B");

    // A->B->A: db316644's whole comparison accepted it; obligations_rev refuses it, and the
    // path then behaves exactly as after the genuine change's refusal.
    let (aba_first, aba_second, aba_w2) = run("aba");
    assert_eq!(aba_first, genuine_first);
    assert_eq!(aba_second, genuine_second);
    assert_eq!(aba_w2.request.condition, "review");
}
