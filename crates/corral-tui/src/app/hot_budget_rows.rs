//! History and nested-history values for the hot-reads budget fixture (plan T-253 section 4,
//! "growth fixture"): every top-level history vector and every nested collection a current
//! container can grow, each sized by an explicit factor. Adapted from corral-core's store test
//! support, which a downstream crate cannot import.

use corral_core::{Run, Task};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

pub(super) const T0: Timestamp = Timestamp::constant(1_767_268_800, 0);

pub(super) fn at(seconds: i64) -> Timestamp {
    T0.checked_add(SignedDuration::from_secs(seconds)).unwrap()
}

fn route() -> Value {
    json!({
        "version": 1,
        "requirement": {
            "contract": {"kind": "implementation", "version": 1},
            "capability": "routine", "checks": [], "reasons": []
        },
        "contract_digest": "contract",
        "signals": {
            "subject": "implementation", "design": false, "kind": null, "size": null,
            "hotfix": false, "flaky": false, "risks": [], "stronger_requested": false
        },
        "profile": "reviewer", "profile_revision": "rev", "profile_source": "task_override",
        "independence": "different_model", "capability_mismatch": false, "round": 1,
        "scope": "full", "stronger_request": null
    })
}

fn usage_entry(i: usize, priced: bool) -> Value {
    if priced {
        json!({"at": at(i as i64), "latency_ms": 10, "model": "m", "input": 10, "output": 5,
               "cache_read": 0, "cache_write": 0, "reported_usd": null,
               "estimated_usd": 0.1, "unpriced_tokens": 0})
    } else {
        json!({"at": null, "latency_ms": null, "model": "m", "input": 0, "output": 0,
               "cache_read": 0, "cache_write": 0, "reported_usd": null, "estimated_usd": null,
               "unpriced_tokens": 0})
    }
}

fn usage(entries: Vec<Value>) -> Value {
    let entries: Vec<corral_core::UsageEntry> = entries
        .into_iter()
        .map(|e| serde_json::from_value(e).unwrap())
        .collect();
    serde_json::to_value(corral_core::Usage::from_entries(entries)).unwrap()
}

fn receipt(run: &str, id: &str) -> Value {
    json!({"version": 1, "id": format!("rc-{id}"), "run": run, "outcome": "answered",
           "delivery": "acknowledged", "observed_at": at(1), "answer": "yes"})
}

/// A final (answered) wait; base and grown runs carry no pending one, so no obligation shows.
fn wait(run: &str, i: usize) -> Value {
    json!({
        "request": {"version": 1, "id": format!("W{i:04}"), "run": run, "condition": "check exited",
                    "owner": "worker", "source": "worker", "deadline": null,
                    "handle": {"kind": "gate", "sha": "a1b2c3d", "pane": "p-7"}},
        "registered_at": at(i as i64),
        "observation": {"at": at(i as i64), "state": "ready", "detail": "observed"},
        "receipt": receipt(run, &format!("W{i:04}"))
    })
}

fn decision(run: &str, i: usize) -> Value {
    json!({
        "request": {"version": 1, "id": format!("D{i:04}"), "run": run, "question": "Ship?",
                    "requested_action": "answer", "owner": "owner", "source": "worker",
                    "deadline": null},
        "registered_at": at(i as i64),
        "observation": {"at": at(i as i64), "state": "ready", "detail": "asked"},
        "receipt": receipt(run, &format!("D{i:04}"))
    })
}

/// Checkpoint `i`; the newest one freezes `frozen` decisions in its run obligations.
fn checkpoint(task: &str, run: &str, i: usize, frozen: usize) -> Value {
    let decisions: Vec<Value> = (0..frozen).map(|d| decision(run, d)).collect();
    json!({
        "artifact": format!("artifacts/{task}/{run}/checkpoint-{i:04}.json"),
        "checkpoint": {
            "version": 1, "revision": i + 1,
            "input": {"task": task, "run": run, "spec_revision": format!("spec-{i:04}"),
                      "expected_revision": i, "progress_revision": i, "head": "1".repeat(40),
                      "base": "0".repeat(40), "tree": format!("{i:064}"),
                      "criterion_ids": [], "evidence_refs": [], "next_action": "continue",
                      "decisions": [], "waits": [], "resources": []},
            "progress": null, "completed_steps": [], "current_step": null,
            "run_obligations": {"decisions": decisions},
            "captured": at(i as i64)
        }
    })
}

fn inbox(effects: usize, cursors: usize) -> Value {
    json!({
        "next_seq": 10000, "next_envelope": 1, "open_envelope": null,
        "effects_applied": (1..=effects).collect::<Vec<_>>(), "binding": null,
        "observe_cursor": {"offset": 0, "last_complete_line": 0, "coverage": "complete"},
        "observe_cursors": (0..cursors).map(|c| json!({
            "adapter": "pi",
            "binding": {"kind": "pi", "session": format!("s-{c}")},
            "cursor": {"offset": c, "last_complete_line": c, "coverage": "complete"}
        })).collect::<Vec<_>>()
    })
}

fn switch(i: usize) -> Value {
    json!({"from": format!("p{i:04}"), "to": format!("p{:04}", i + 1), "reason": "switch"})
}

/// A coding run or review child with `nested` of each nested collection. Its worktree never
/// exists, so the set of existing worktrees stays equal across variants.
fn run_value(task: &str, id: &str, started: i64, nested: usize, ended: bool) -> Value {
    let mut entries: Vec<Value> = (0..nested).map(|i| usage_entry(i, false)).collect();
    entries.push(usage_entry(nested, true));
    let checkpoints: Vec<Value> = (0..nested.max(1))
        .map(|i| checkpoint(task, id, i, if i + 1 == nested.max(1) { nested } else { 1 }))
        .collect();
    json!({
        "id": id, "agent": "worker", "harness": "omo",
        "worktree": format!("/nonexistent/corral-hot-budget/{task}/{id}"),
        "branch": format!("corral/{task}"), "base": "main", "base_sha": "0".repeat(40),
        "started": at(started),
        "ended": if ended { json!(at(started + 1)) } else { Value::Null },
        "outcome": if ended { json!("done") } else { Value::Null },
        "usage": usage(entries),
        "checkpoints": checkpoints,
        "switches": (0..nested).map(switch).collect::<Vec<_>>(),
        "obligations": {
            "waits": (0..nested).map(|i| wait(id, i)).collect::<Vec<_>>(),
            "decisions": (0..nested).map(|i| decision(id, i)).collect::<Vec<_>>()
        },
        "inbox": inbox(nested, nested),
    })
}

fn review_child(task: &str, run: &str, n: usize, ended: bool) -> Value {
    let mut child = run_value(
        task,
        &format!("{run}-review{n:04}"),
        100 + n as i64,
        1,
        ended,
    );
    child["usage"] = usage(vec![usage_entry(0, false)]);
    child["review"] = json!({
        "reviewer": "reviewer", "verdict": "approve", "notes": null, "sha": "abc",
        "at": at(200 + n as i64), "route": route()
    });
    child
}

/// A coding run with `children` approved review children; `ended` closes the run and its last
/// child.
fn coding_run(
    task: &str,
    id: &str,
    started: i64,
    children: usize,
    nested: usize,
    ended: bool,
) -> Run {
    let mut run = run_value(task, id, started, nested, ended);
    run["review_runs"] = (1..=children)
        .map(|n| review_child(task, id, n, n < children || ended))
        .collect::<Vec<_>>()
        .into();
    serde_json::from_value(run).unwrap()
}

fn plan_review(id: &str, verdict: Option<&str>) -> Value {
    json!({"id": id, "reviewer": "rev", "policy_digest": "policy", "verdict": verdict,
           "notes": null, "at": at(300), "inbox": inbox(1, 1)})
}

/// A plan whose last two reviews are the base's; `earlier` approved reviews precede them.
fn plan(task: &str, id: &str, earlier: usize) -> Value {
    let mut reviews: Vec<Value> = (1..=earlier)
        .map(|r| plan_review(&format!("{id}-PR{r:04}"), Some("approve")))
        .collect();
    reviews.push(plan_review(&format!("{id}-PRB01"), Some("changes")));
    reviews.push(plan_review(&format!("{id}-PRB02"), Some("approve")));
    json!({
        "id": id,
        "artifact": {"run": "P1", "kind": "plan", "title": "Plan", "path": format!("artifacts/{task}/P1/plan.md"),
                     "source": "/nonexistent/plan.md", "sha256": "0".repeat(64), "bytes": 10, "added": at(0)},
        "spec_revision": "spec",
        "document": {"goal": "Goal", "non_goals": [], "decisions": [], "affected_paths": [],
                     "contracts": [], "dependencies": [], "acceptance": [], "verification": [],
                     "owner_decisions": []},
        "reviews": reviews,
    })
}

fn identity(task: &str) -> Value {
    json!({"version": 2, "task": task, "run": "aggregate", "attempt": "A1", "head": "1".repeat(40),
           "tree": "2".repeat(64), "integration_base": "0".repeat(40), "dispatch_base": "0".repeat(40),
           "spec_digest": "s", "policy_digest": "p", "inputs_digest": "i"})
}

fn aggregate_review(task: &str, id: &str) -> Value {
    json!({"id": id, "reviewer": "rev", "route": route(), "identity": identity(task),
           "evidence_digest": "e", "requested": at(400), "verdict": "approve", "at": at(401)})
}

fn attempt(task: &str, id: &str, earlier: usize) -> Value {
    json!({"id": id, "identity": identity(task), "target": {"kind": "main"}, "code": false,
           "members": [], "started": at(0),
           "reviews": (1..=earlier + 2)
               .map(|r| aggregate_review(task, &format!("{id}-R{r:04}")))
               .collect::<Vec<_>>()})
}

fn plan_run(id: &str) -> Value {
    json!({"id": id, "profile": "planner", "harness": "omo", "model": null, "vendor": null,
           "effort": null, "spec_revision": "spec", "started": at(0), "ended": at(1),
           "output_dir": "/nonexistent/plans", "submitted": null, "rounds": 0, "state": "planning",
           "pending_reply": null, "inbox": inbox(1, 1)})
}

/// Grows `task` to `history` of each top-level vector, the last coding run current (open when
/// `open`) with `nested` of each nested collection and `nested` earlier approved review
/// children; the latest plan and aggregate attempt get `nested - 1` earlier approved reviews
/// before the base's last two.
pub(super) fn grow(task: &mut Task, history: usize, nested: usize, open: bool) {
    let id = task.id.clone();
    task.runs = (1..=history)
        .map(|r| {
            let current = r == history;
            coding_run(
                &id,
                &format!("R{r:04}"),
                r as i64 * 10,
                if current { nested + 1 } else { 1 },
                if current { nested } else { 1 },
                !(current && open),
            )
        })
        .collect();
    task.plan_runs = (1..=history)
        .map(|r| serde_json::from_value(plan_run(&format!("P{r:04}"))).unwrap())
        .collect();
    let earlier = |last: bool| if last { nested - 1 } else { 0 };
    task.plans = (1..=history)
        .map(|p| {
            serde_json::from_value(plan(&id, &format!("P{p:04}"), earlier(p == history))).unwrap()
        })
        .collect();
    task.aggregate_attempts = (1..=history)
        .map(|a| {
            serde_json::from_value(attempt(&id, &format!("A{a:04}"), earlier(a == history)))
                .unwrap()
        })
        .collect();
    task.comments = (0..history)
        .map(|c| {
            serde_json::from_value(json!({"author": "owner", "at": at(c as i64),
                "text": format!("note {c:04}")}))
            .unwrap()
        })
        .collect();
    let current = format!("R{history:04}");
    task.artifacts = (0..history)
        .map(|a| {
            serde_json::from_value(json!({"run": current, "kind": "log", "title": format!("log {a:04}"),
                "path": format!("artifacts/{id}/{current}/{a:04}.log"), "source": "/nonexistent/x.log",
                "sha256": "0".repeat(64), "bytes": 1, "added": at(a as i64)}))
            .unwrap()
        })
        .collect();
}
