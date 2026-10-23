//! `corral stats --gates`: cohort, tier and destination rows over gate events.
use corral_core::{Event, Run, Status, Store, Task};
use corral_ops::analytics::gate_metrics::collect_gate_metrics;
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
    let mut oracle = |_: &Task, _: &str, head: &str| (head != "unknown").then_some(head == "new");
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
