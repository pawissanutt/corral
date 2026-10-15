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
