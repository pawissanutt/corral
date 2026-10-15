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
