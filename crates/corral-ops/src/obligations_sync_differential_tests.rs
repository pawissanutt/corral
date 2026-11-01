//! The obligations tick against `db316644`'s loop (plan T-253 section 4, "Obligations"): at
//! randomized times, re-observing only the open-obligations page stores the same obligations
//! and logs the same transition events as re-observing every wait and decision, and its work
//! is identical whether the run holds 1x or 10x final waits and answered decisions.
use corral_core::work::{self, Counter, HotScope};
use corral_core::{Event, Run, RunObligations, Status, Store};
use jiff::Timestamp;
use serde_json::{Value, json};

use super::*;

fn at(seconds: i64) -> Timestamp {
    "2026-10-19T00:00:00Z"
        .parse::<Timestamp>()
        .unwrap()
        .checked_add(jiff::SignedDuration::from_secs(seconds))
        .unwrap()
}

fn receipt(id: &str) -> Value {
    json!({"version": 1, "id": format!("rc-{id}"), "run": "R1", "outcome": "answered",
           "delivery": "acknowledged", "observed_at": at(0), "answer": "yes"})
}

fn rendered(id: &str) -> String {
    format!("receipt rc-{id}: Answered; delivery Acknowledged")
}

/// A wait; with `receipt_of` final, its observation `normalized` to its receipt's or not yet.
fn wait(
    id: &str,
    deadline: Option<i64>,
    capability: Option<&str>,
    receipt_of: Option<bool>,
) -> Value {
    let mut wait = json!({
        "request": {"version": 1, "id": id, "run": "R1", "condition": format!("{id} done"),
                    "owner": "worker", "source": "worker", "deadline": deadline.map(at),
                    "handle": {"kind": "channel", "channel_id": format!("c-{id}"),
                               "capability": capability}},
        "registered_at": at(0),
        "observation": {"at": at(0), "state": "pending", "detail": "registered"}
    });
    if let Some(normalized) = receipt_of {
        let detail = if normalized {
            rendered(id)
        } else {
            "answered".into()
        };
        wait["receipt"] = receipt(id);
        wait["observation"] = json!({"at": at(0), "state": "ready", "detail": detail});
    }
    wait
}

fn decision(id: &str, deadline: Option<i64>, answered: bool) -> Value {
    let mut decision = json!({
        "request": {"version": 1, "id": id, "run": "R1", "question": format!("{id}?"),
                    "requested_action": "answer", "owner": "owner", "source": "worker",
                    "deadline": deadline.map(at)},
        "registered_at": at(0),
        "observation": {"at": at(0), "state": "pending", "detail": "asked"}
    });
    if answered {
        decision["receipt"] = receipt(id);
        decision["observation"] = json!({"at": at(0), "state": "ready", "detail": "answered"});
    }
    decision
}

/// Open waits and decisions with deadlines across the sampled window, behind `finals` final
/// waits and answered decisions.
fn store(root: &std::path::Path, finals: usize, normalized: bool) -> (Ctx, String) {
    corral_core::clock::pin_thread(Some(at(0)));
    let ctx = Ctx {
        store: Store::init(root).unwrap(),
        herdr: corral_herdr::Herdr::with_bin(root.join("no-herdr"), None),
        resource_probe: crate::resources::FixedResourceProbe::arc(),
    };
    ctx.store.fixture_pin_now(at(0)).unwrap();
    let task = ctx
        .store
        .create("Waits", None, Status::Running, "")
        .unwrap();
    let mut waits: Vec<Value> = (0..finals)
        .map(|i| wait(&format!("F{i:03}"), None, Some("wait_v1"), Some(normalized)))
        .collect();
    waits.extend([
        wait("W-soon", Some(100), Some("wait_v1"), None),
        wait("W-late", Some(900), Some("wait_v1"), None),
        wait("W-unbounded", None, Some("wait_v1"), None),
        wait("W-unsupported", Some(500), None, None),
    ]);
    let mut decisions: Vec<Value> = (0..finals)
        .map(|i| decision(&format!("A{i:03}"), None, true))
        .collect();
    decisions.extend([
        decision("D-soon", Some(300), false),
        decision("D-late", Some(700), false),
        decision("D-open", None, false),
    ]);
    let run: Run = serde_json::from_value(json!({
        "id": "R1", "agent": "claude", "worktree": root, "branch": "main", "base": "main",
        "base_sha": "fixture", "started": at(0),
        "obligations": {"waits": waits, "decisions": decisions}
    }))
    .unwrap();
    let mut task = ctx.store.get(&task.id).unwrap();
    task.runs.push(run);
    // Fixed stamps: wall-clock ones print with a varying number of digits.
    task.created = at(0);
    task.updated = at(0);
    ctx.store.fixture_put_task(&task).unwrap();
    assert_eq!(corral_core::clock::now(), at(0));
    assert_eq!(ctx.store.now(), at(0));
    assert_eq!(ctx.store.head(&task.id).unwrap().status_since, Some(at(0)));
    (ctx, task.id)
}

/// `db316644`'s tick: every wait and decision re-observed from the full task, written back
/// whole under the whole-obligations guard.
fn reference_tick(ctx: &Ctx, id: &str, now: Timestamp) {
    let task = ctx.store.get(id).unwrap();
    let Some(run) = task.current_run().filter(|r| r.outcome.is_none()) else {
        return;
    };
    if !task.status.holds_slot() || run.obligations.is_empty() {
        return;
    }
    let mut next = run.obligations.clone();
    let mut transitions = Vec::new();
    for wait in &mut next.waits {
        let (state, detail, receipt) = observe(ctx, &task.id, run, wait, now, None);
        if let Some(receipt) = receipt {
            wait.receipt = Some(receipt);
        }
        if wait.observation.state != state || wait.observation.detail != detail {
            let changed_state = wait.observation.state != state;
            wait.observation = WaitObservation {
                at: now,
                state,
                detail,
            };
            if changed_state {
                transitions.push(
                    json!({"wait":wait.request.id,"state":state,"condition":wait.request.condition}),
                );
            }
        }
    }
    for decision in &mut next.decisions {
        if decision.pending()
            && decision.request.deadline.is_some_and(|at| now >= at)
            && decision.observation.state != WaitState::Expired
        {
            decision.observation = WaitObservation {
                at: now,
                state: WaitState::Expired,
                detail: "owner decision deadline expired; answer still required".into(),
            };
            transitions.push(json!({"decision":decision.request.id,"state":"expired","question":decision.request.question}));
        }
    }
    if next == run.obligations {
        return;
    }
    let previous = run.obligations.clone();
    ctx.store
        .try_update(id, |t| {
            let current = t.current_run_mut().unwrap();
            if current.obligations != previous {
                return Ok(false);
            }
            current.obligations = next.clone();
            Ok(true)
        })
        .unwrap();
    for detail in transitions {
        transition(ctx, id, "wait_transition", detail).unwrap();
    }
}

fn state(ctx: &Ctx, id: &str) -> (RunObligations, Vec<Value>) {
    let obligations = ctx.store.get(id).unwrap().runs[0].obligations.clone();
    let events: Vec<Event> = ctx.store.events_of(Some(id), &["wait_transition"]).unwrap();
    (obligations, events.into_iter().map(|e| e.detail).collect())
}

/// Whole-second sample times from a fixed seed: deterministic, unevenly spaced, ascending.
fn times(seed: u64) -> Vec<Timestamp> {
    let mut x = seed;
    let mut t = 0;
    (0..12)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            t += 1 + i64::try_from((x >> 33) % 160).unwrap();
            at(t)
        })
        .collect()
}

#[test]
fn the_open_page_decides_as_the_loop_over_every_obligation() {
    for seed in [1, 7, 42, 1009] {
        let tmp = tempfile::tempdir().unwrap();
        // Final waits whose observations the first tick still normalizes, as the loop does.
        let (ours, id) = store(&tmp.path().join("ours"), 10, false);
        let (reference, _) = store(&tmp.path().join("reference"), 10, false);
        for now in times(seed) {
            reconcile_one(&ours, &id, now, None).unwrap();
            reference_tick(&reference, &id, now);
            assert_eq!(
                state(&ours, &id),
                state(&reference, &id),
                "seed {seed} at {now}"
            );
        }
        let (obligations, events) = state(&ours, &id);
        assert!(
            !events.is_empty(),
            "seed {seed}: the sampled window crosses deadlines"
        );
        assert_eq!(obligations.waits.len(), 14);
    }
}

fn unattributed(ctx: &Ctx, id: &str, now: Timestamp) -> Vec<(&'static str, u64)> {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    reconcile_one(ctx, id, now, None).unwrap();
    let delta = work::snapshot().since(&before);
    // Notice claims are decoded again during this same pass. They must use the
    // fixture's clock too, rather than a raw wall-clock timestamp of variable width.
    let head = ctx.store.head(id).unwrap();
    assert!(!head.attention.notice_outbox.is_empty());
    for claim in &head.attention.notice_outbox {
        assert_eq!(claim.claimed_at, at(0));
    }
    Counter::ALL
        .iter()
        .map(|c| (c.name(), delta.cells(*c).unattributed))
        .collect()
}

#[test]
fn its_work_does_not_grow_with_final_waits_and_answered_decisions() {
    let tmp = tempfile::tempdir().unwrap();
    let (one, id) = store(&tmp.path().join("one"), 1, true);
    let (ten, _) = store(&tmp.path().join("ten"), 10, true);
    // A tick that expires a wait and writes it, then a tick with nothing to change.
    for now in [at(150), at(160)] {
        assert_eq!(
            unattributed(&one, &id, now),
            unattributed(&ten, &id, now),
            "at {now}"
        );
    }
}
