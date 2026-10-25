//! Rich tasks for the database-store tests: every history vector and every nested collection a
//! current container can grow, each sized by an explicit factor.
#![allow(dead_code)]

use std::collections::BTreeMap;

use corral_core::backend::{HistoryRow, TaskNum, TaskPart, TaskParts};
use corral_core::{Run, Status, Store, Task};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

pub type Rows = BTreeMap<(TaskPart, String), HistoryRow>;

pub const T0: &str = "2026-01-01T00:00:00Z";

pub fn at(seconds: i64) -> Timestamp {
    T0.parse::<Timestamp>()
        .unwrap()
        .checked_add(SignedDuration::from_secs(seconds))
        .unwrap()
}

pub fn route(stronger: bool) -> Value {
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
        "scope": "full",
        "stronger_request": if stronger { json!("stronger please") } else { Value::Null }
    })
}

pub fn usage_entry(i: usize, priced: bool) -> Value {
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

pub fn usage(entries: Vec<Value>) -> Value {
    let entries: Vec<corral_core::UsageEntry> = entries
        .into_iter()
        .map(|e| serde_json::from_value(e).unwrap())
        .collect();
    serde_json::to_value(corral_core::Usage::from_entries(entries)).unwrap()
}

pub fn wait(run: &str, i: usize, answered: bool) -> Value {
    json!({
        "request": {"version": 1, "id": format!("W{i:04}"), "run": run, "condition": "check exited",
                    "owner": "worker", "source": "worker", "deadline": null,
                    "handle": {"kind": "gate", "sha": "a1b2c3d", "pane": "p-7"}},
        "registered_at": at(i as i64),
        "observation": {"at": at(i as i64), "state": if answered { "ready" } else { "pending" },
                         "detail": "observed"},
        "receipt": if answered { receipt(run, &format!("W{i:04}")) } else { Value::Null }
    })
}

pub fn decision(run: &str, i: usize, answered: bool) -> Value {
    json!({
        "request": {"version": 1, "id": format!("D{i:04}"), "run": run, "question": "Ship?",
                    "requested_action": "answer", "owner": "owner", "source": "worker",
                    "deadline": null},
        "registered_at": at(i as i64),
        "observation": {"at": at(i as i64), "state": if answered { "ready" } else { "pending" },
                         "detail": "asked"},
        "receipt": if answered { receipt(run, &format!("D{i:04}")) } else { Value::Null }
    })
}

fn receipt(run: &str, id: &str) -> Value {
    json!({"version": 1, "id": format!("rc-{id}"), "run": run, "outcome": "answered",
           "delivery": "acknowledged", "observed_at": at(1), "answer": "yes"})
}

pub fn checkpoint(task: &str, run: &str, i: usize, frozen: usize) -> Value {
    let decisions: Vec<Value> = (0..frozen).map(|d| decision(run, d, true)).collect();
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

pub fn inbox(effects: usize, cursors: usize) -> Value {
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

pub fn switch(i: usize) -> Value {
    json!({"from": format!("p{i:04}"), "to": format!("p{:04}", i + 1), "reason": "switch"})
}

/// A coding run or review child with `nested` of each nested collection.
pub fn run_value(task: &str, id: &str, started: i64, nested: usize, ended: bool) -> Value {
    let mut entries: Vec<Value> = (0..nested).map(|i| usage_entry(i, false)).collect();
    entries.push(usage_entry(nested, true));
    let checkpoints: Vec<Value> = (0..nested.max(1))
        .map(|i| checkpoint(task, id, i, if i + 1 == nested.max(1) { nested } else { 1 }))
        .collect();
    let mut waits: Vec<Value> = (0..nested).map(|i| wait(id, i, true)).collect();
    waits.push(wait(id, nested, false));
    let mut decisions: Vec<Value> = (0..nested).map(|i| decision(id, i, true)).collect();
    decisions.push(decision(id, nested, false));
    json!({
        "id": id, "agent": "worker", "harness": "omo",
        "worktree": format!("/tmp/corral-fixture/{task}/{id}"),
        "branch": format!("corral/{task}"), "base": "main", "base_sha": "0".repeat(40),
        "started": at(started),
        "ended": if ended { json!(at(started + 1)) } else { Value::Null },
        "outcome": if ended { json!("done") } else { Value::Null },
        "herdr": {"workspace_id": "w1", "pane_id": format!("p-{id}"), "socket": "/tmp/herdr.sock"},
        "usage": usage(entries),
        "checkpoints": checkpoints,
        "switches": (0..nested).map(switch).collect::<Vec<_>>(),
        "obligations": {"waits": waits, "decisions": decisions},
        "inbox": inbox(nested, nested),
    })
}

pub fn review_child(
    task: &str,
    run: &str,
    n: usize,
    nested: usize,
    ended: bool,
    verdict: &str,
) -> Value {
    let mut child = run_value(
        task,
        &format!("{run}-review{n:04}"),
        100 + n as i64,
        nested,
        ended,
    );
    child["review"] = json!({
        "reviewer": "reviewer", "verdict": verdict, "notes": null, "sha": "abc",
        "at": at(200 + n as i64), "route": route(n.is_multiple_of(3))
    });
    if n.is_multiple_of(3) && ended {
        child["outcome"] = json!("superseded");
    }
    child
}

/// A coding run with `children` review children, the last one open.
pub fn coding_run(
    task: &str,
    id: &str,
    started: i64,
    children: usize,
    nested: usize,
    ended: bool,
) -> Run {
    let mut run = run_value(task, id, started, nested, ended);
    run["review_runs"] = (1..=children)
        .map(|n| {
            let last = n == children;
            let mut child = review_child(
                task,
                id,
                n,
                if last { 1 } else { nested.min(2) },
                !last || ended,
                "approve",
            );
            if !last {
                child["usage"] = usage(vec![usage_entry(0, false)]);
            }
            child
        })
        .collect::<Vec<_>>()
        .into();
    serde_json::from_value(run).unwrap()
}

pub fn plan_review(id: &str, reviewer: &str, verdict: Option<&str>) -> Value {
    json!({"id": id, "reviewer": reviewer, "policy_digest": "policy", "verdict": verdict,
           "notes": null, "at": at(300), "inbox": inbox(1, 1)})
}

pub fn plan(task: &str, id: &str, reviews: usize) -> Value {
    json!({
        "id": id,
        "artifact": {"run": "P1", "kind": "plan", "title": "Plan", "path": format!("artifacts/{task}/P1/plan.md"),
                     "source": "/tmp/plan.md", "sha256": "0".repeat(64), "bytes": 10, "added": at(0)},
        "spec_revision": "spec",
        "document": {"goal": "Goal", "non_goals": [], "decisions": [], "affected_paths": [],
                     "contracts": [], "dependencies": [], "acceptance": [], "verification": [],
                     "owner_decisions": []},
        "reviews": (1..=reviews).map(|r| plan_review(&format!("{id}-PR{r:04}"),
            if r + 1 >= reviews { "rev" } else { "unnamed" },
            if r == reviews { None } else { Some("approve") })).collect::<Vec<_>>(),
    })
}

pub fn identity(task: &str) -> Value {
    json!({"version": 2, "task": task, "run": "aggregate", "attempt": "A1", "head": "1".repeat(40),
           "tree": "2".repeat(64), "integration_base": "0".repeat(40), "dispatch_base": "0".repeat(40),
           "spec_digest": "s", "policy_digest": "p", "inputs_digest": "i"})
}

pub fn aggregate_review(id: &str, pending: bool) -> Value {
    json!({"id": id, "reviewer": "rev", "route": route(false), "identity": identity("T-001"),
           "evidence_digest": "e", "requested": at(400),
           "verdict": if pending { Value::Null } else { json!("approve") },
           "at": if pending { Value::Null } else { json!(at(401)) }})
}

pub fn attempt(task: &str, id: &str, reviews: usize) -> Value {
    json!({"id": id, "identity": identity(task), "target": {"kind": "main"}, "code": false,
           "members": [], "started": at(0),
           "reviews": (1..=reviews).map(|r| aggregate_review(&format!("{id}-R{r:04}"), r == reviews))
               .collect::<Vec<_>>()})
}

pub fn plan_run(id: &str, ended: bool) -> Value {
    json!({"id": id, "profile": "planner", "harness": "omo", "model": null, "vendor": null,
           "effort": null, "spec_revision": "spec", "started": at(0),
           "ended": if ended { json!(at(1)) } else { Value::Null },
           "herdr": {"workspace_id": "w1", "pane_id": format!("p-{id}"), "socket": null},
           "output_dir": "/tmp/plans", "submitted": null, "rounds": 0, "state": "planning",
           "pending_reply": null, "inbox": inbox(1, 1)})
}

/// Grows `task` to `runs` coding runs (the last current with `children` review children),
/// `plans` plans and plan runs, attempts, comments, artifacts and scope revisions, each nested
/// collection of the current containers sized `nested`.
pub fn grow(task: &mut Task, runs: usize, children: usize, history: usize, nested: usize) {
    let id = task.id.clone();
    task.runs = (1..=runs)
        .map(|r| {
            let current = r == runs;
            coding_run(
                &id,
                &format!("R{r:04}"),
                r as i64 * 10,
                if current { children } else { 2 },
                if current { nested } else { 1 },
                !current,
            )
        })
        .collect();
    task.plan_runs = (1..=history)
        .map(|r| serde_json::from_value(plan_run(&format!("P{r:04}"), r < history)).unwrap())
        .collect();
    task.plans = (1..=history)
        .map(|p| {
            serde_json::from_value(plan(
                &id,
                &format!("P{p:04}"),
                if p == history { nested + 2 } else { 1 },
            ))
            .unwrap()
        })
        .collect();
    task.aggregate_attempts = (1..=history)
        .map(|a| {
            serde_json::from_value(attempt(
                &id,
                &format!("A{a:04}"),
                if a == history { nested + 2 } else { 1 },
            ))
            .unwrap()
        })
        .collect();
    task.comments = (0..history)
        .map(|c| {
            serde_json::from_value(
                json!({"author": "owner", "at": at(c as i64), "text": format!("comment {c:04}")}),
            )
            .unwrap()
        })
        .collect();
    task.artifacts = (0..history)
        .map(|a| {
            serde_json::from_value(
                json!({"run": "R0001", "kind": "log", "title": format!("log {a:04}"),
            "path": format!("artifacts/{id}/R0001/{a:04}.log"), "source": "/tmp/x.log",
            "sha256": "0".repeat(64), "bytes": 1, "added": at(a as i64)}),
            )
            .unwrap()
        })
        .collect();
    task.approved_scope_history = (0..history)
        .map(|r| {
            serde_json::from_value(
                json!({"revision": r, "approved_scope": null, "superseded_at": at(r as i64)}),
            )
            .unwrap()
        })
        .collect();
}

pub fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    (dir, store)
}

/// A task grown as [`grow`] says and stored through the full write path.
pub fn rich(store: &Store, runs: usize, children: usize, history: usize, nested: usize) -> Task {
    let mut task = store
        .create("Rich task", Some("app"), Status::Running, "Spec body\n")
        .unwrap();
    grow(&mut task, runs, children, history, nested);
    store.fixture_put_task(&task).unwrap();
    store.get(&task.id).unwrap()
}

/// A deterministic xorshift generator for the randomized store tests.
pub struct Rng(pub u64);

impl Rng {
    pub fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// Every stored row of task T-001, by family and key.
pub fn task_rows(store: &Store) -> Rows {
    store
        .view()
        .unwrap()
        .task(TaskNum(1), &TaskParts::All)
        .unwrap()
        .unwrap()
        .rows
        .into_iter()
        .map(|row| ((row.part, row.key.clone()), row))
        .collect()
}

pub fn revision(row: &HistoryRow, field: &str) -> u64 {
    let value = &row.doc["_facts"][field];
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .unwrap()
}

pub fn changed_under(before: &Rows, after: &Rows, part: TaskPart, prefix: &str) -> bool {
    let docs = |rows: &Rows| -> Vec<(String, Value)> {
        rows.iter()
            .filter(|((p, key), _)| *p == part && key.starts_with(prefix))
            .map(|((_, key), row)| (key.clone(), row.doc.clone()))
            .collect()
    };
    docs(before) != docs(after)
}
