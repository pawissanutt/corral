//! `corral stats --gates`: cohort, tier and destination rows over gate events.
use corral_core::{Event, Run, Status, Store, Task};
use std::collections::HashMap;

use corral_ops::analytics::gate_metrics::{GateSources, collect_gate_metrics, parse_step_markers};
use jiff::Timestamp;
use serde_json::{Value, json};

fn at(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn run(id: &str) -> Run {
    serde_json::from_value(json!({
        "id":id, "agent":"implementer", "worktree":"/unused", "branch":"fixture",
        "base":"main", "base_sha":"abc", "started":"2026-01-01T00:00:00Z"
    }))
    .unwrap()
}

fn gate_event(task: &str, kind: &str, time: &str, actor: &str, detail: Value) -> Event {
    serde_json::from_value(
        json!({"task":task, "kind":kind, "ts":time, "actor":actor, "detail":detail}),
    )
    .unwrap()
}

fn started(
    task: &str,
    run: &str,
    time: &str,
    attempt: &str,
    tier: Option<&str>,
    head: &str,
) -> Event {
    let mut detail =
        json!({"run":run, "identity":{"attempt":attempt, "integration_base":"base", "head":head}});
    if let Some(tier) = tier {
        detail["tier"] = json!(tier);
    }
    gate_event(task, "gate_started", time, "agent", detail)
}

fn finished(task: &str, run: &str, time: &str, attempt: &str, failed: Option<&str>) -> Event {
    let commands = json!([{"command":"cargo test", "exit_code":0, "log_start":0, "log_end":1,
        "started":"2026-01-01T00:00:00Z", "ended":"2026-01-01T00:01:30Z"}]);
    gate_event(
        task,
        "gate",
        time,
        "system",
        json!({"run":run, "identity":{"attempt":attempt},
        "result": if failed.is_some() {"fail"} else {"pass"}, "failed_cmd":failed, "commands":commands}),
    )
}

#[test]
fn gate_metrics_split_cohorts_tiers_and_destinations_and_count_legacy_as_full() {
    // Given: a legacy run, a tiered main-bound run and a tiered Plan member, with carries.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut tasks = Vec::new();
    for (title, rules, target) in [
        ("Legacy", "legacy", false),
        ("Main", "tiered", false),
        ("Member", "tiered", true),
    ] {
        let mut task = store.create(title, Some("app"), Status::Done, "").unwrap();
        let mut dispatch = run("R1");
        dispatch.gate_rules = serde_json::from_value(json!(rules)).unwrap();
        if target {
            dispatch.plan_target = Some(
                serde_json::from_value(json!({"plan":"T-9", "scope_revision":1,
                "contract_revision":"c", "branch":"corral/plan/T-9", "checkout":"/unused"}))
                .unwrap(),
            );
        }
        task.runs = vec![dispatch];
        store.save(&task).unwrap();
        tasks.push(task.id);
    }
    let (legacy, main, member) = (&tasks[0], &tasks[1], &tasks[2]);
    let events = vec![
        started(legacy, "R1", "2026-01-01T00:00:00Z", "a1", None, "old"),
        finished(legacy, "R1", "2026-01-01T00:06:00Z", "a1", None),
        gate_event(
            legacy,
            "approved",
            "2026-01-01T01:00:00Z",
            "owner",
            json!({"run":"R1", "strategy":"rebase"}),
        ),
        started(
            main,
            "R1",
            "2026-01-02T00:00:00Z",
            "b1",
            Some("quick"),
            "new",
        ),
        finished(
            main,
            "R1",
            "2026-01-02T00:03:00Z",
            "b1",
            Some("stale_result: candidate changed"),
        ),
        gate_event(
            main,
            "candidate_rebased",
            "2026-01-02T00:04:00Z",
            "autopilot",
            json!({"run":"R1"}),
        ),
        started(
            main,
            "R1",
            "2026-01-02T00:05:00Z",
            "b2",
            Some("quick"),
            "new",
        ),
        finished(main, "R1", "2026-01-02T00:08:00Z", "b2", None),
        gate_event(
            main,
            "autopilot",
            "2026-01-02T00:09:00Z",
            "autopilot",
            json!({"run":"R1", "to":"gating", "note":"reusing the gate run on this commit"}),
        ),
        gate_event(
            main,
            "verdict",
            "2026-01-02T00:20:00Z",
            "reviewer",
            json!({"run":"R1", "verdict":"approve"}),
        ),
        gate_event(
            main,
            "verdict_carried",
            "2026-01-02T00:30:00Z",
            "autopilot",
            json!({"run":"R1", "method":"tier_promotion", "evidence_carry":{"method":"tier_promotion"}}),
        ),
        gate_event(
            main,
            "approved",
            "2026-01-02T01:00:00Z",
            "autopilot",
            json!({"run":"R1", "strategy":"rebase"}),
        ),
        started(
            member,
            "R1",
            "2026-01-03T00:00:00Z",
            "c1",
            Some("quick"),
            "new",
        ),
        finished(member, "R1", "2026-01-03T00:02:00Z", "c1", None),
        gate_event(
            member,
            "gate_carried",
            "2026-01-03T00:30:00Z",
            "autopilot",
            json!({"run":"R1"}),
        ),
        gate_event(
            member,
            "approved",
            "2026-01-03T01:00:00Z",
            "owner",
            json!({"run":"R1", "strategy":"keep_branch"}),
        ),
        finished(member, "R1", "2026-01-03T02:00:00Z", "zz", None),
    ];
    let mut oracle = Fixture::default();
    // When: the pure collector reports gates over the whole window.
    let report = serde_json::to_value(collect_gate_metrics(
        &store.config,
        &store.list().unwrap(),
        &events,
        None,
        None,
        &mut oracle,
    ))
    .unwrap();
    // Then: rows separate cohort, tier and destination; untiered events are tier full.
    let groups = report["groups"].as_array().unwrap();
    let keys: Vec<_> = groups
        .iter()
        .map(|g| {
            (
                g["cohort"].as_str().unwrap(),
                g["tier"].as_str().unwrap(),
                g["destination"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        keys,
        [
            ("plan_member", "quick", "app:corral/plan/T-9"),
            ("main_bound_tiered", "quick", "app:main"),
            ("legacy_or_untiered", "full", "app:main")
        ]
    );
    let quick = &groups[1];
    assert_eq!(quick["gates_started"], 2);
    assert_eq!(quick["classes"]["stale_result"], 1);
    assert_eq!(quick["stale_result_share"], 0.5);
    assert_eq!(quick["landed_tasks"], 1);
    assert_eq!(quick["gates_per_landed_task"], 2.0);
    assert_eq!(quick["gate_hours_per_landed_task"], 0.1);
    assert_eq!(quick["review_rounds"], 1);
    assert_eq!(quick["gates_per_review_round"], 2.0);
    assert_eq!(quick["followed_by_rebase"], 1);
    assert_eq!(quick["worker_receipts_reused_by_autopilot"], 1);
    assert!(
        quick.get("full_step_seconds").is_none(),
        "step seconds are full-tier only"
    );
    let full = &groups[2];
    assert_eq!(full["behind"]["behind_at_start"]["gates"], 1);
    assert_eq!(
        full["full_step_seconds"]["cargo test"]["median_seconds"],
        90.0
    );
    // keep_branch moves no ref, so the member did not land.
    assert_eq!(groups[0]["landed_tasks"], 0);
    assert_eq!(report["gate_events_unmatched"], 1);
    assert_eq!(report["totals"]["gates_started"], 4);
    assert_eq!(report["totals"]["landed_tasks"], 2);
    assert_eq!(report["totals"]["behind"]["contained_base"]["gates"], 3);
    let carries = report["carries"].as_array().unwrap();
    assert_eq!(carries[0]["cohort"], "plan_member");
    assert_eq!(
        (&carries[0]["from_tier"], &carries[0]["to_tier"]),
        (&json!("full"), &json!("full"))
    );
    assert_eq!(carries[0]["gate"]["clean_replay"], 1);
    assert_eq!(carries[1]["verdict"]["tier_promotion"], 1);
    assert_eq!(carries[1]["evidence"]["tier_promotion"], 1);
    // And: the window bounds both ends.
    let windowed = collect_gate_metrics(
        &store.config,
        &store.list().unwrap(),
        &events,
        Some(at("2026-01-02T00:00:00Z")),
        Some(at("2026-01-02T23:00:00Z")),
        &mut oracle,
    );
    assert_eq!(windowed.totals.gates_started, 2);
    assert_eq!(windowed.groups.len(), 1);
}

/// Ancestry by head name and gate logs by attempt.
#[derive(Default)]
struct Fixture {
    logs: HashMap<String, Vec<u8>>,
}

impl GateSources for Fixture {
    fn contains(&mut self, _: &Task, _: &str, head: &str) -> Option<bool> {
        (head != "unknown").then_some(head == "new")
    }

    fn steps(&mut self, _: &str, _: &str, attempt: &str) -> Option<Vec<(String, f64)>> {
        let steps = parse_step_markers(self.logs.get(attempt)?);
        (!steps.is_empty()).then_some(steps)
    }
}

fn tiered_tasks(store: &Store, count: usize) -> Vec<String> {
    (0..count)
        .map(|i| {
            let mut task = store
                .create(&format!("Main {i}"), Some("app"), Status::Done, "")
                .unwrap();
            let mut dispatch = run("R1");
            dispatch.gate_rules = serde_json::from_value(json!("tiered")).unwrap();
            task.runs = vec![dispatch];
            store.save(&task).unwrap();
            task.id
        })
        .collect()
}

fn report(store: &Store, events: &[Event], fixture: &mut Fixture, since: Option<&str>) -> Value {
    serde_json::to_value(collect_gate_metrics(
        &store.config,
        &store.list().unwrap(),
        events,
        since.map(at),
        None,
        fixture,
    ))
    .unwrap()
}

#[test]
fn full_step_seconds_come_from_gate_log_markers_including_a_failed_step() {
    // Given: a full gate wrapped in one command whose log times each step, one of which failed,
    // and a second full gate whose log has no markers.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let id = store
        .create("Steps", Some("app"), Status::Done, "")
        .unwrap()
        .id;
    let mut fixture = Fixture::default();
    fixture.logs.insert(
        "w1".into(),
        b"$ bash -c 'scripts/gate.sh'\n==> tier full\n\n==> cargo fmt --all -- --check\n<== cargo fmt --all -- --check 1.853s\n==> cargo clippy\nerror: lint\n<== cargo clippy 10.5s\nFAILED: cargo clippy\n<== not a marker\n".to_vec(),
    );
    let events = vec![
        started(&id, "R1", "2026-01-01T00:00:00Z", "w1", None, "new"),
        finished(
            &id,
            "R1",
            "2026-01-01T00:01:30Z",
            "w1",
            Some("cargo clippy"),
        ),
        started(&id, "R1", "2026-01-01T01:00:00Z", "w2", None, "new"),
        finished(&id, "R1", "2026-01-01T01:01:30Z", "w2", None),
    ];
    // When: the report collects full-gate steps.
    let report = report(&store, &events, &mut fixture, None);
    // Then: each logged step is its own sample, and only the marker-free gate falls back to
    // its command receipt.
    for steps in [
        &report["totals"]["full_step_seconds"],
        &report["groups"][0]["full_step_seconds"],
    ] {
        let names: Vec<&String> = steps.as_object().unwrap().keys().collect();
        assert_eq!(
            names,
            ["cargo clippy", "cargo fmt --all -- --check", "cargo test"]
        );
        assert_eq!(steps["cargo fmt --all -- --check"]["total_seconds"], 1.853);
        assert_eq!(steps["cargo clippy"]["median_seconds"], 10.5);
        assert_eq!(steps["cargo test"]["samples"], 1);
    }
}

#[test]
fn carries_keep_source_and_destination_tiers_in_one_cohort_and_destination() {
    // Given: one main-bound run with a quick-to-full promotion, a quick gate carry and a legacy
    // verdict carry without tiers.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let id = tiered_tasks(&store, 1).remove(0);
    let events = vec![
        gate_event(
            &id,
            "verdict_carried",
            "2026-01-01T00:00:00Z",
            "autopilot",
            json!({"run":"R1",
            "method":"tier_promotion", "from_tier":"quick", "to_tier":"full",
            "evidence_carry":{"method":"tier_promotion"}}),
        ),
        gate_event(
            &id,
            "gate_carried",
            "2026-01-01T00:01:00Z",
            "autopilot",
            json!({"run":"R1",
            "tier":"quick", "evidence_carry":{"method":"clean_replay"}}),
        ),
        gate_event(
            &id,
            "verdict_carried",
            "2026-01-01T00:02:00Z",
            "autopilot",
            json!({"run":"R1"}),
        ),
    ];
    // When: the report groups carries.
    let report = report(&store, &events, &mut Fixture::default(), None);
    // Then: each tier pair is its own group with gate, verdict and evidence counts.
    let carries = report["carries"].as_array().unwrap();
    let keys: Vec<_> = carries
        .iter()
        .map(|c| {
            (
                c["from_tier"].as_str().unwrap(),
                c["to_tier"].as_str().unwrap(),
                c["destination"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        keys,
        [
            ("full", "full", "app:main"),
            ("quick", "full", "app:main"),
            ("quick", "quick", "app:main")
        ]
    );
    assert!(carries.iter().all(|c| c["cohort"] == "main_bound_tiered"));
    assert_eq!(carries[0]["verdict"]["clean_replay"], 1);
    assert_eq!(carries[0]["evidence"], json!({}));
    assert_eq!(carries[1]["verdict"]["tier_promotion"], 1);
    assert_eq!(carries[1]["evidence"]["tier_promotion"], 1);
    assert_eq!(carries[2]["gate"]["clean_replay"], 1);
    assert_eq!(carries[2]["evidence"]["clean_replay"], 1);
}

#[test]
fn landings_and_verdicts_count_in_their_group_when_gates_precede_the_window() {
    // Given: two main-bound tasks that both land after a full gate and a verdict inside the
    // window, one gated inside it and one before it, and a legacy task gated only before it.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let ids = tiered_tasks(&store, 2);
    let legacy = store
        .create("Legacy", Some("app"), Status::Done, "")
        .unwrap();
    let mut legacy_run = run("R1");
    legacy_run.base = "release".into();
    let mut legacy = legacy;
    legacy.runs = vec![legacy_run];
    store.save(&legacy).unwrap();
    let land = |task: &str, time: &str| {
        gate_event(
            task,
            "approved",
            time,
            "autopilot",
            json!({"run":"R1", "strategy":"rebase"}),
        )
    };
    let verdict = |task: &str, time: &str| {
        gate_event(
            task,
            "verdict",
            time,
            "reviewer",
            json!({"run":"R1", "verdict":"approve"}),
        )
    };
    let events = vec![
        started(
            &ids[1],
            "R1",
            "2026-01-01T00:00:00Z",
            "b1",
            Some("full"),
            "new",
        ),
        finished(&ids[1], "R1", "2026-01-01T00:06:00Z", "b1", None),
        started(&legacy.id, "R1", "2026-01-01T00:10:00Z", "c1", None, "new"),
        finished(&legacy.id, "R1", "2026-01-01T00:16:00Z", "c1", None),
        started(
            &ids[0],
            "R1",
            "2026-01-02T00:00:00Z",
            "a1",
            Some("full"),
            "new",
        ),
        finished(&ids[0], "R1", "2026-01-02T00:06:00Z", "a1", None),
        verdict(&ids[0], "2026-01-02T00:10:00Z"),
        verdict(&ids[1], "2026-01-02T00:11:00Z"),
        land(&ids[0], "2026-01-02T01:00:00Z"),
        land(&ids[1], "2026-01-02T01:01:00Z"),
        land(&legacy.id, "2026-01-02T01:02:00Z"),
    ];
    // When: the window starts after the earlier gates.
    let report = report(
        &store,
        &events,
        &mut Fixture::default(),
        Some("2026-01-02T00:00:00Z"),
    );
    // Then: both landings and verdicts stay in the full main-bound group, and the gate-free
    // legacy landing keeps its own group.
    let groups = report["groups"].as_array().unwrap();
    let full = groups
        .iter()
        .find(|g| g["cohort"] == "main_bound_tiered")
        .unwrap();
    assert_eq!(
        (&full["tier"], &full["destination"]),
        (&json!("full"), &json!("app:main"))
    );
    assert_eq!(full["gates_started"], 1);
    assert_eq!(full["landed_tasks"], 2);
    assert_eq!(full["review_rounds"], 2);
    assert_eq!(full["gates_per_landed_task"], 0.5);
    assert_eq!(full["gate_hours_per_landed_task"], 0.05);
    assert_eq!(full["gates_per_review_round"], 0.5);
    let gate_free = groups
        .iter()
        .find(|g| g["cohort"] == "legacy_or_untiered")
        .unwrap();
    assert_eq!(
        (&gate_free["tier"], &gate_free["destination"]),
        (&json!("full"), &json!("app:release"))
    );
    assert_eq!(
        (&gate_free["gates_started"], &gate_free["landed_tasks"]),
        (&json!(0), &json!(1))
    );
    assert_eq!(report["totals"]["landed_tasks"], 3);
    assert_eq!(report["totals"]["review_rounds"], 2);
}

#[test]
fn promoted_runs_count_their_landing_and_review_round_for_quick_and_full_gates() {
    // Given: one main-bound run with a worker quick gate reused by autopilot, an independent
    // review, a full landing gate, a quick-to-full verdict and evidence carry, and a landing.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let id = tiered_tasks(&store, 1).remove(0);
    let events = vec![
        started(
            &id,
            "R1",
            "2026-01-02T00:00:00Z",
            "q1",
            Some("quick"),
            "new",
        ),
        finished(&id, "R1", "2026-01-02T00:06:00Z", "q1", None),
        gate_event(
            &id,
            "autopilot",
            "2026-01-02T00:07:00Z",
            "autopilot",
            json!({"run":"R1", "to":"gating", "note":"reusing the gate run on this commit"}),
        ),
        gate_event(
            &id,
            "verdict",
            "2026-01-02T00:20:00Z",
            "reviewer",
            json!({"run":"R1", "verdict":"approve"}),
        ),
        started(&id, "R1", "2026-01-02T00:30:00Z", "f1", Some("full"), "new"),
        finished(&id, "R1", "2026-01-02T00:36:00Z", "f1", None),
        gate_event(
            &id,
            "verdict_carried",
            "2026-01-02T00:37:00Z",
            "autopilot",
            json!({"run":"R1",
            "method":"tier_promotion", "from_tier":"quick", "to_tier":"full",
            "evidence_carry":{"method":"tier_promotion"}}),
        ),
        gate_event(
            &id,
            "approved",
            "2026-01-02T01:00:00Z",
            "autopilot",
            json!({"run":"R1", "strategy":"rebase"}),
        ),
    ];
    let tier = |report: &Value, tier: &str| {
        report["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["tier"] == tier)
            .unwrap()
            .clone()
    };
    let ratios = |g: &Value| {
        (
            g["gates_started"].clone(),
            g["landed_tasks"].clone(),
            g["gates_per_landed_task"].clone(),
            g["gate_hours_per_landed_task"].clone(),
            g["review_rounds"].clone(),
            g["gates_per_review_round"].clone(),
        )
    };
    // When: the window holds the whole run.
    let whole = report(
        &store,
        &events,
        &mut Fixture::default(),
        Some("2026-01-02T00:00:00Z"),
    );
    // Then: each tier carries its own cost per landed task and per review round.
    let expected = (
        json!(1),
        json!(1),
        json!(1.0),
        json!(0.1),
        json!(1),
        json!(1.0),
    );
    assert_eq!(ratios(&tier(&whole, "quick")), expected);
    assert_eq!(ratios(&tier(&whole, "full")), expected);
    assert_eq!(
        tier(&whole, "quick")["worker_receipts_reused_by_autopilot"],
        1
    );
    let totals = &whole["totals"];
    assert_eq!(
        (
            &totals["landed_tasks"],
            &totals["gates_per_landed_task"],
            &totals["gate_hours_per_landed_task"]
        ),
        (&json!(1), &json!(2.0), &json!(0.2))
    );
    assert_eq!(
        (&totals["review_rounds"], &totals["gates_per_review_round"]),
        (&json!(1), &json!(2.0))
    );
    // When: the window starts after the quick gate.
    let bounded = report(
        &store,
        &events,
        &mut Fixture::default(),
        Some("2026-01-02T00:10:00Z"),
    );
    // Then: the quick group keeps the outcomes with no in-window quick cost, and the full
    // group still owns its landing gate.
    assert_eq!(
        ratios(&tier(&bounded, "quick")),
        (
            json!(0),
            json!(1),
            json!(0.0),
            json!(0.0),
            json!(1),
            json!(0.0)
        )
    );
    assert_eq!(ratios(&tier(&bounded, "full")), expected);
}
