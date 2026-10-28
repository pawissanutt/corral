//! Random tasks with findings, duplicate review times and agent hosts, stored on both backends,
//! and the normalizations that compare a head's projections with the full task's.
#![allow(dead_code)]

use std::sync::Arc;

use corral_core::agent::AgentRunState;
use corral_core::backend::fake::FakeBackend;
use corral_core::backend::{BackendConfig, FaultInjector, ManualClock};
use corral_core::{AttemptHead, PlanHead, PlanRunHead, RunHead, Status, Store, Task, Verdict};
use serde_json::json;

use crate::store_support::{Rng, at, grow, store};

pub fn backends() -> Vec<(&'static str, tempfile::TempDir, Store)> {
    let (sqlite_dir, sqlite) = store();
    let (fake_dir, base) = store();
    let backend = FakeBackend::new(
        BackendConfig::default(),
        Arc::new(ManualClock::new(jiff::Timestamp::now())),
        Arc::new(FaultInjector::default()),
    );
    let fake = base.with_backend(Box::new(backend));
    vec![("sqlite", sqlite_dir, sqlite), ("fake", fake_dir, fake)]
}

pub const CALLBACK: &str = "gate:callback";

pub fn receipt(request: &str, at_s: i64) -> serde_json::Value {
    json!({"version": 1, "request_id": request, "op": "event", "payload_sha256": "0".repeat(64),
           "identity": {"store": "s", "project": "app", "role": "worker", "task": "T-001",
                        "run": "R0001", "session_id": "s", "generation": 1},
           "state": "committed", "result": {"kind": "turn_started"}, "at": at(at_s)})
}

fn question(container: &str, n: usize, state: &str) -> serde_json::Value {
    json!({"version": 1, "key": format!("{container}/Q{n}"), "revision": 1,
           "asked": {"request_id": format!("q-{n}"), "payload_sha256": "0".repeat(64),
                     "session_id": "s", "generation": 1, "at": at(20)},
           "kind": "clarification", "blocking": false, "text": format!("question {n}"),
           "state": state,
           "wake": {"route": "none", "key": format!("qwake:{container}/Q{n}:1"), "state": "pending"}})
}

fn wake(n: usize, state: &str) -> serde_json::Value {
    json!({"key": format!("wake-{n}"), "request_id": format!("w-{n}"), "kind": "wait",
           "target": format!("W{n:04}"), "state": state, "at": at(30)})
}

fn report(n: usize) -> serde_json::Value {
    json!({"version": 1, "kind": "done", "request_id": format!("r-{n}"), "generation": 1,
           "at": at(40)})
}

pub fn agent_state(role: &str, generation: u32, fenced: bool, callback: bool) -> AgentRunState {
    agent_state_with(role, generation, fenced, callback, "T-001/R", 2)
}

pub fn agent_state_with(
    role: &str,
    generation: u32,
    fenced: bool,
    callback: bool,
    container: &str,
    records: usize,
) -> AgentRunState {
    let generations: Vec<_> = (1..=generation)
        .map(|n| {
            let fence = (fenced && n == generation).then(|| json!({"reason": "lost", "at": at(5)}));
            json!({"n": n, "bind_nonce_sha256": "0".repeat(64), "pane": format!("p-{n}"),
                   "created": at(i64::from(n)), "fenced": fence})
        })
        .collect();
    let turns: Vec<_> = (1..=2u64)
        .map(|seq| {
            json!({"generation": generation, "seq": seq, "origin": {"kind": "bridge"},
                   "started": at(10), "ended": if seq == 1 { json!(at(11)) } else { json!(null) }})
        })
        .collect();
    let states = ["open", "answered", "escalated", "withdrawn"];
    let wake_states = ["armed", "fired", "pending_start", "cancelled"];
    serde_json::from_value(json!({
        "version": 1,
        "binding": {"role": role, "store": "s", "project": "app", "generation": generation,
                    "health": "healthy", "callback": if callback { json!(CALLBACK) } else { json!(null) }},
        "generations": generations,
        "turns": turns,
        "receipts": (0..records).map(|n| receipt(&format!("rq-{n}"), n as i64)).collect::<Vec<_>>(),
        "questions": (0..records).map(|n| question(container, n, states[n % 4])).collect::<Vec<_>>(),
        "wakes": (0..records).map(|n| wake(n, wake_states[n % 4])).collect::<Vec<_>>(),
        "reports": (0..records).map(report).collect::<Vec<_>>(),
    }))
    .unwrap()
}

fn member_gate(revision: u64, history: u64) -> serde_json::Value {
    let entry = |r: u64| {
        json!({"revision": r, "gate": {"tier": "quick"}, "reason": format!("rev {r}"),
               "by": "owner", "at": at(r as i64)})
    };
    let mut gate = entry(revision);
    gate["history"] = (0..history).map(entry).collect::<Vec<_>>().into();
    gate
}

fn findings(task: &mut Task, rng: &mut Rng) {
    for run in &mut task.runs {
        for child in &mut run.review_runs {
            let Some(review) = child.review.as_mut() else {
                continue;
            };
            match rng.below(5) {
                0 => {
                    review.verdict = Some(Verdict::Changes);
                    review.notes = Some(format!("fix {}", child.id));
                }
                1 => review.notes = Some("looks fine".into()),
                2 => {
                    review.verdict = Some(Verdict::Escalate);
                    review.notes = Some("owner".into());
                }
                3 => review.verdict = Some(Verdict::Changes),
                _ => {}
            }
        }
    }
    for plan in &mut task.plans {
        for review in &mut plan.reviews {
            (review.verdict, review.notes) = match rng.below(5) {
                0 => (
                    Some(Verdict::Changes),
                    Some("Resolved: Ship? => yes".into()),
                ),
                1 => (Some(Verdict::Escalate), None),
                2 => (Some(Verdict::Approve), Some("ok".into())),
                3 => (None, None),
                _ => (review.verdict, review.notes.take()),
            };
        }
    }
}

pub fn random_task(store: &Store, rng: &mut Rng) -> Task {
    let mut task = store
        .create("Property", Some("app"), Status::Running, "Spec\n")
        .unwrap();
    let runs = 1 + rng.below(3);
    let children = 1 + rng.below(4);
    grow(&mut task, runs, children, 1 + rng.below(3), rng.below(3));
    findings(&mut task, rng);
    task.status = [
        Status::Running,
        Status::Review,
        Status::NeedsMe,
        Status::Done,
        Status::Dropped,
    ][rng.below(5)];
    let current = task.runs.last_mut().unwrap();
    if rng.below(3) == 0 {
        current.outcome = Some(
            [
                corral_core::RunOutcome::Done,
                corral_core::RunOutcome::Superseded,
                corral_core::RunOutcome::Discarded,
            ][rng.below(3)],
        );
    }
    if rng.below(2) == 0 {
        current.blocked_excerpt = Some("Question Q1 needs the owner".into());
    }
    let generation = 1 + rng.below(2) as u32;
    current.agent_host = (rng.below(4) != 0)
        .then(|| Box::new(agent_state("worker", generation, rng.below(3) == 0, false)));
    let n = current.review_runs.len();
    if n >= 2 && rng.below(2) == 0 {
        let at = current.review_runs[n - 2].review.as_ref().unwrap().at;
        current.review_runs[n - 1].review.as_mut().unwrap().at = at;
    }
    if let Some(review) = current.review.as_mut().filter(|_| rng.below(2) == 0) {
        review.process_lost = true;
    }
    for child in current.review_runs.iter_mut() {
        if rng.below(2) == 0 {
            child.agent_host = Some(Box::new(agent_state(
                "reviewer",
                generation,
                rng.below(4) == 0,
                rng.below(3) != 0,
            )));
        }
    }
    task.agent_pm_receipts = (0..rng.below(3))
        .map(|n| serde_json::from_value(receipt(&format!("pm-{n}"), n as i64)).unwrap())
        .collect();
    for (member, history) in [("T-090", rng.below(3) as u64), ("T-091", 0)] {
        task.member_gates.insert(
            member.into(),
            serde_json::from_value(member_gate(history + 1, history)).unwrap(),
        );
    }
    store.fixture_put_task(&task).unwrap();
    store.get(&task.id).unwrap()
}

pub fn norm_run(mut run: RunHead) -> RunHead {
    run.facts = run.facts.without_revisions();
    run.current_child = run.current_child.map(|c| Box::new(norm_run(*c)));
    run
}

pub fn norm_plan_run(mut run: PlanRunHead) -> PlanRunHead {
    run.facts = run.facts.without_revisions();
    run
}

pub fn norm_plan(mut plan: PlanHead) -> PlanHead {
    plan.facts = plan.facts.without_revisions();
    plan
}

pub fn norm_attempt(mut attempt: AttemptHead) -> AttemptHead {
    attempt.facts = attempt.facts.without_revisions();
    attempt
}
