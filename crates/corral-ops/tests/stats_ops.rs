use corral_core::{Run, Status, Store};
use corral_ops::{StatsBy, stats};
use serde_json::json;

fn run(id: &str, start: &str, model: Option<&str>) -> Run {
    serde_json::from_value(json!({
        "id": id, "agent": "fast", "model": model,
        "worktree": "/unused", "branch": "test", "base": "main",
        "base_sha": "abc", "started": start
    }))
    .unwrap()
}

fn lesson() -> corral_core::Lesson {
    serde_json::from_value(json!({
        "id":"L1", "rule":"Check every gate attempt", "why":"Retain failure evidence",
        "confidence":0.8, "last_seen":"2025-12-30T00:00:00Z"
    }))
    .unwrap()
}

#[test]
fn mixed_runs_attribute_legacy_events_and_keep_all_gate_attempts() {
    // Given two dispatch intervals and a second task with no outcome evidence.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store.create("Fix", Some("app"), Status::Done, "").unwrap();
    task.kind = Some("bug".into());
    task.runs = vec![
        run("R1", "2026-01-01T00:00:00Z", Some("small")),
        run("R2", "2026-01-02T00:00:00Z", Some("large")),
    ];
    store.save(&task).unwrap();
    let mut other = store.create("Pending", None, Status::Running, "").unwrap();
    other.runs = vec![run("R1", "2026-01-03T00:00:00Z", None)];
    store.save(&other).unwrap();
    let events = [
        ("2026-01-01T00:01:00Z", "changes_requested", json!({})),
        ("2026-01-01T00:02:00Z", "discarded", json!({"run":"R1"})),
        (
            "2026-01-02T00:01:00Z",
            "gate",
            json!({"run":"R2","result":"fail"}),
        ),
        (
            "2026-01-02T00:02:00Z",
            "gate",
            json!({"run":"R2","result":"pass"}),
        ),
        ("2026-01-02T00:03:00Z", "done", json!({"run":"R2"})),
        ("2026-01-02T00:04:00Z", "approved", json!({"run":"R2"})),
    ];
    let lines: String = events
        .into_iter()
        .map(|(ts, kind, detail)| {
            format!(
                "{}\n",
                json!({"ts":ts,"task":task.id,"kind":kind,"detail":detail})
            )
        })
        .collect();
    std::fs::write(store.dir().join("events.jsonl"), lines).unwrap();
    // When aggregated by profile.
    let rows = stats(&store, StatsBy::Profile, None).unwrap();
    // Then unresolved runs do not depress the acceptance rate.
    let row = &rows[0];
    assert_eq!(row.runs, 3);
    assert_eq!(row.accepted, 1);
    assert_eq!(row.first_round_accept_rate, Some(0.5));
    assert_eq!(row.mean_rounds_to_accept, Some(1.0));
    assert_eq!(row.gate_failure_rate, Some(0.5));
    assert_eq!(row.changes_requested, 1);
    assert_eq!(row.discards, 1);
    assert_eq!(row.median_wall_seconds, Some(180.0));
    let models = stats(
        &store,
        StatsBy::Model,
        Some("2026-01-02T00:00:00Z".parse().unwrap()),
    )
    .unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].group, "large");
    assert_eq!(models[0].first_round_accept_rate, Some(1.0));
    assert_eq!(models[1].group, "unknown");
    assert_eq!(models[1].first_round_accept_rate, None);
    assert_eq!(stats(&store, StatsBy::Kind, None).unwrap()[0].group, "bug");
    assert_eq!(
        stats(&store, StatsBy::Project, None).unwrap()[0].group,
        "app"
    );
}

#[test]
fn accepted_run_counts_repeated_reviews_and_even_median() {
    // Given two approvals, one after two requests, with 60s and 180s durations.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    for (seconds, changes) in [(60, 0), (180, 2)] {
        let mut task = store.create("Work", None, Status::Done, "").unwrap();
        let mut r = run("R1", "2026-01-01T00:00:00Z", None);
        r.outcome = Some(corral_core::RunOutcome::Done);
        r.ended = Some(
            r.started
                .checked_add(jiff::SignedDuration::from_secs(seconds))
                .unwrap(),
        );
        task.runs.push(r);
        store.save(&task).unwrap();
        for _ in 0..changes {
            store
                .log_event(Some(&task.id), "changes_requested", json!({"run":"R1"}))
                .unwrap();
        }
        store
            .log_event(Some(&task.id), "approved", json!({"run":"R1"}))
            .unwrap();
    }
    // When stats are read from the fixture store.
    let rows = stats(&store, StatsBy::Profile, None).unwrap();
    // Then rounds and median include both accepted samples.
    assert_eq!(rows[0].mean_rounds_to_accept, Some(2.0));
    assert_eq!(rows[0].median_wall_seconds, Some(120.0));
    assert_eq!(rows[0].first_round_accept_rate, Some(0.5));
}

#[test]
fn usage_aggregates_known_samples_including_zero_with_independent_cost_coverage() {
    // Given missing usage, known zero usage, and cache-inclusive samples.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store.create("Usage", None, Status::Done, "").unwrap();
    for (index, usage) in [
        serde_json::Value::Null,
        json!({"input":0,"output":0,"cache_read":0,"cache_write":0,"cost_usd":0.0}),
        json!({"input":10,"output":20,"cache_read":30,"cache_write":40}),
        json!({"input":20,"output":40,"cache_read":60,"cache_write":80,"cost_usd":0.5}),
        json!({"input":30,"output":60,"cache_read":90,"cache_write":120,"cost_usd":1.0}),
    ]
    .into_iter()
    .enumerate()
    {
        let mut value = serde_json::to_value(run(
            &format!("R{index}"),
            "2026-01-02T00:00:00Z",
            Some("known"),
        ))
        .unwrap();
        value["usage"] = usage;
        value["ended"] = json!("2026-01-02T00:01:00Z");
        task.runs.push(serde_json::from_value(value).unwrap());
    }
    task.runs
        .push(run("old", "2026-01-01T00:00:00Z", Some("excluded")));
    task.runs
        .push(run("missing", "2026-01-02T00:00:00Z", Some("missing")));
    store.save(&task).unwrap();
    // When selecting a dispatch cohort and grouping by model.
    let rows = stats(
        &store,
        StatsBy::Model,
        Some("2026-01-02T00:00:00Z".parse().unwrap()),
    )
    .unwrap();
    // Then missing samples do not enter either denominator; zero samples do.
    assert_eq!(rows.len(), 2);
    let known = &rows[0];
    assert_eq!(known.runs, 5);
    assert_eq!(known.usage_runs, Some(4));
    assert_eq!(known.cost_runs, Some(3));
    assert_eq!(known.total_tokens, Some(600));
    assert_eq!(known.median_tokens_per_run, Some(150.0));
    assert_eq!(known.total_cost_usd, Some(1.5));
    assert_eq!(known.median_cost_usd_per_run, Some(0.5));
    let missing = serde_json::to_value(&rows[1]).unwrap();
    for field in [
        "median_tokens_per_run",
        "total_tokens",
        "usage_runs",
        "median_cost_usd_per_run",
        "total_cost_usd",
        "cost_runs",
    ] {
        assert!(missing.get(field).is_none(), "{field}");
    }
    let known = serde_json::to_value(known).unwrap();
    assert_eq!(known["usage_runs"], 4);
    assert_eq!(known["cost_runs"], 3);
}

#[test]
fn token_only_zero_usage_has_no_cost_statistics() {
    // Given a known zero-token run without cost evidence.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store.create("Zero", None, Status::Done, "").unwrap();
    let mut value = serde_json::to_value(run("R1", "2026-01-01T00:00:00Z", None)).unwrap();
    value["usage"] = json!({"input":0,"output":0,"cache_read":0,"cache_write":0});
    value["ended"] = json!("2026-01-01T00:01:00Z");
    task.runs.push(serde_json::from_value(value).unwrap());
    store.save(&task).unwrap();
    // When aggregated.
    let rows = stats(&store, StatsBy::Profile, None).unwrap();
    // Then zero remains known and missing cost is omitted.
    assert_eq!(rows[0].median_tokens_per_run, Some(0.0));
    assert_eq!(rows[0].total_tokens, Some(0));
    assert_eq!(rows[0].usage_runs, Some(1));
    let value = serde_json::to_value(&rows[0]).unwrap();
    for field in ["median_cost_usd_per_run", "total_cost_usd", "cost_runs"] {
        assert!(value.get(field).is_none(), "{field}");
    }
}

#[test]
fn task_costs_include_reviewers_without_counting_them_as_dispatches() {
    // Given two dispatches, one with two reviewer attempts and mixed cost evidence.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.pricing.insert(
        "priced".into(),
        serde_json::from_value(json!({"input":10000.0,"output":0.0})).unwrap(),
    );
    let mut task = store.create("Reviewed", None, Status::Done, "").unwrap();
    let mut first = serde_json::to_value(run("R1", "2026-01-01T00:00:00Z", None)).unwrap();
    let mut reviewer = first.clone();
    reviewer["id"] = json!("review-1");
    reviewer["agent"] = json!("reviewer");
    reviewer["model"] = json!("priced");
    reviewer["ended"] = json!("2026-01-01T00:01:00Z");
    reviewer["usage"] = json!({
        "input":20,"output":0,"cache_read":0,"cache_write":0,
        "cost_usd":0.05
    });
    first["ended"] = reviewer["ended"].clone();
    first["usage"] = json!({
        "input":10,"output":0,"cache_read":0,"cache_write":0,
        "cost_usd":0.1,"reported_usd":0.1,"cost_basis":"reported"
    });
    first["review_runs"] = json!([reviewer, reviewer]);
    let mut second = first.clone();
    second["id"] = json!("R2");
    second["model"] = json!("priced");
    second["review_runs"] = json!([]);
    second["usage"] = json!({
        "input":30,"output":0,"cache_read":0,"cache_write":0,
        "estimated_usd":0.6,"cost_basis":"estimated"
    });
    task.runs = vec![
        serde_json::from_value(first).unwrap(),
        serde_json::from_value(second).unwrap(),
    ];
    store.save(&task).unwrap();
    store
        .log_event(Some(&task.id), "approved", json!({"run":"R1"}))
        .unwrap();

    // When grouped by task over dispatched runs.
    let rows = stats(&store, StatsBy::Task, None).unwrap();

    // Then reviewer tokens and costs enter per-dispatch samples, not outcome counts.
    let row = &rows[0];
    assert_eq!(row.group, task.id);
    assert_eq!(row.runs, 2);
    assert_eq!(row.accepted, 1);
    assert_eq!(row.first_round_accept_rate, Some(1.0));
    assert_eq!(row.total_tokens, Some(80));
    assert_eq!(row.median_tokens_per_run, Some(40.0));
    assert_eq!(row.usage_runs, Some(2));
    assert_eq!(row.cost_runs, Some(1));
    assert_eq!(row.estimated_runs, Some(2));
    assert!((row.total_cost_usd.unwrap() - 0.2).abs() < 1e-9);
    assert!((row.total_estimated_usd.unwrap() - 0.7).abs() < 1e-9);
    assert!((row.median_estimated_usd_per_run.unwrap() - 0.35).abs() < 1e-9);
    assert_eq!(row.unpriced_tokens, Some(10));
    assert_eq!(serde_json::to_value(row).unwrap()["cost_basis"], "mixed");
}

#[test]
fn lesson_cohorts_preserve_unknown_exposure_and_outcome_denominators() {
    // Given snapshots before dispatch, legacy outcomes, and repeated gates.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Lessons", Some("app"), Status::Done, "")
        .unwrap();
    task.runs = vec![
        run("old", "2025-12-31T00:00:00Z", None),
        run("R1", "2026-01-01T00:00:00Z", None),
        run("R2", "2026-01-02T00:00:00Z", None),
        run("R3", "2026-01-03T00:00:00Z", None),
        run("R4", "2026-01-04T00:00:00Z", None),
        run("R5", "2026-01-05T00:00:00Z", None),
    ];
    task.runs[4].gate = Some(
        serde_json::from_value(json!({
            "result":"fail", "at":"2026-01-04T00:01:00Z", "sha":"abc"
        }))
        .unwrap(),
    );
    store.save(&task).unwrap();
    let mut other = store.create("Legacy", None, Status::Done, "").unwrap();
    other.runs = vec![run("R1", "2026-01-01T00:00:00Z", None)];
    store.save(&other).unwrap();
    let lesson = lesson();
    let events = [
        (
            "2025-12-30T00:00:00Z",
            "lessons_applied",
            json!({"run":"old","lessons":[lesson],"project":"app"}),
        ),
        ("2025-12-31T00:01:00Z", "approved", json!({"run":"old"})),
        (
            "2025-12-31T23:59:59Z",
            "lessons_applied",
            json!({"run":"R1","lessons":[lesson],"project":"app"}),
        ),
        ("2026-01-01T00:01:00Z", "changes_requested", json!({})),
        (
            "2026-01-01T00:02:00Z",
            "changes_requested",
            json!({"run":"R1"}),
        ),
        (
            "2026-01-01T00:03:00Z",
            "gate",
            json!({"run":"R1","result":"fail"}),
        ),
        (
            "2026-01-01T00:04:00Z",
            "gate",
            json!({"run":"R1","result":"fail"}),
        ),
        (
            "2026-01-01T00:05:00Z",
            "gate",
            json!({"run":"R1","result":"pass"}),
        ),
        ("2026-01-01T00:06:00Z", "approved", json!({"run":"R1"})),
        (
            "2026-01-01T23:59:59Z",
            "lessons_applied",
            json!({"run":"R2","lessons":[],"project":"app"}),
        ),
        (
            "2026-01-02T00:01:00Z",
            "gate",
            json!({"run":"R2","result":"pass"}),
        ),
        ("2026-01-20T00:00:00Z", "approved", json!({"run":"R2"})),
        ("2026-01-03T00:01:00Z", "changes_requested", json!({})),
        ("2026-01-03T00:02:00Z", "gate", json!({"result":"fail"})),
        ("2026-01-03T00:03:00Z", "discarded", json!({})),
        (
            "2026-01-03T23:59:59Z",
            "lessons_applied",
            json!({"run":"R4","lessons":[lesson],"project":"app"}),
        ),
        (
            "2026-01-04T00:01:00Z",
            "changes_requested",
            json!({"run":"R4"}),
        ),
        (
            "2026-01-04T23:59:59Z",
            "lessons_applied",
            json!({"run":"R5","lessons":[],"project":"app"}),
        ),
    ];
    let mut lines: String = events
        .into_iter()
        .map(|(ts, kind, detail)| {
            format!(
                "{}\n",
                json!({"ts":ts,"task":task.id,"kind":kind,"detail":detail})
            )
        })
        .collect();
    lines.push_str(&format!(
        "{}\n",
        json!({"ts":"2026-01-01T00:01:00Z","task":other.id,"kind":"approved","detail":{"run":"R1"}})
    ));
    std::fs::write(store.dir().join("events.jsonl"), lines).unwrap();

    // When selecting an inclusive dispatch cohort without truncating its outcomes.
    let rows = stats(
        &store,
        StatsBy::Profile,
        Some("2026-01-01T00:00:00Z".parse().unwrap()),
    )
    .unwrap();

    // Then empty and missing snapshots remain distinct, with raw denominators.
    let row = &rows[0];
    assert_eq!(row.runs, 6);
    assert_eq!(row.accepted, 3);
    assert_eq!(row.changes_requested, 4);
    assert_eq!(
        serde_json::to_value(&row.lesson_effectiveness).unwrap(),
        json!({
            "exposed": {
                "runs":2, "accepted":1, "mean_rounds_to_accept":3.0,
                "gate_failure_rate":0.75, "gate_attempts":4, "gate_failures":3,
                "changes_requested":3, "rounds_to_accept":3
            },
            "unexposed": {
                "runs":2, "accepted":1, "mean_rounds_to_accept":1.0,
                "gate_failure_rate":0.0, "gate_attempts":1, "gate_failures":0,
                "changes_requested":0, "rounds_to_accept":1
            },
            "unknown": {
                "runs":2, "accepted":1, "mean_rounds_to_accept":1.0,
                "gate_failure_rate":1.0, "gate_attempts":1, "gate_failures":1,
                "changes_requested":1, "rounds_to_accept":1
            }
        })
    );
}

#[test]
fn lesson_cohorts_obey_cutoff_and_keep_empty_denominators_unknown() {
    // Given an old exposed dispatch and a selected unresolved unexposed dispatch.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store.create("Cutoff", None, Status::Running, "").unwrap();
    task.runs = vec![
        run("old", "2026-01-01T00:00:00Z", Some("old")),
        run("new", "2026-01-02T00:00:00Z", Some("new")),
    ];
    store.save(&task).unwrap();
    store
        .log_event(
            Some(&task.id),
            "lessons_applied",
            json!({"run":"old","lessons":[lesson()],"project":null}),
        )
        .unwrap();
    store
        .log_event(Some(&task.id), "approved", json!({"run":"old"}))
        .unwrap();
    store
        .log_event(
            Some(&task.id),
            "lessons_applied",
            json!({"run":"new","lessons":[],"project":null}),
        )
        .unwrap();

    // When grouping only dispatches at or after the exact cutoff.
    let rows = stats(
        &store,
        StatsBy::Model,
        Some("2026-01-02T00:00:00Z".parse().unwrap()),
    )
    .unwrap();

    // Then excluded events cannot create a group or enter a denominator.
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].group, "new");
    let cohorts = &rows[0].lesson_effectiveness;
    assert_eq!(cohorts.exposed.runs, 0);
    assert_eq!(cohorts.unknown.runs, 0);
    assert_eq!(cohorts.unexposed.runs, 1);
    assert_eq!(cohorts.unexposed.accepted, 0);
    assert_eq!(cohorts.unexposed.mean_rounds_to_accept, None);
    assert_eq!(cohorts.unexposed.gate_failure_rate, None);
    assert_eq!(cohorts.unexposed.gate_attempts, 0);
    assert_eq!(cohorts.unexposed.rounds_to_accept, 0);
}
