use corral_core::{Event, Run, Status, Store, Task, Usage, UsageEntry};
use corral_ops::{StatsBy, collect_analytics, stats};
use jiff::Timestamp;
use serde_json::{Value, json};

fn at(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn run(id: &str, day: u8) -> Run {
    serde_json::from_value(json!({
        "id":id, "agent":"implementer", "harness":"custom", "model":"impl-model",
        "effort":"high", "worktree":"/unused", "branch":"fixture",
        "base":"main", "base_sha":"abc",
        "started":format!("2026-01-{day:02}T00:00:00Z"),
        "ended":format!("2026-01-{day:02}T01:00:00Z")
    }))
    .unwrap()
}

fn usage(tokens: [u64; 4], reported: Option<f64>, estimate: Option<f64>) -> Usage {
    Usage::from_entries(vec![UsageEntry {
        model: Some("fixture".into()),
        input: tokens[0],
        output: tokens[1],
        cache_read: tokens[2],
        cache_write: tokens[3],
        reported_usd: reported,
        estimated_usd: estimate,
        unpriced_tokens: if estimate.is_some() {
            0
        } else {
            tokens.iter().sum()
        },
        ..Default::default()
    }])
}

fn event(task: &Task, run: &str, kind: &str, time: &str, detail: Value) -> Event {
    serde_json::from_value(json!({
        "task":task.id, "kind":kind, "ts":time,
        "detail":{"run":run, "result":detail["result"], "strategy":detail["strategy"]}
    }))
    .unwrap()
}

fn collect(store: &Store) -> Value {
    serde_json::to_value(
        collect_analytics(
            &store.list().unwrap(),
            &store.events().unwrap(),
            None,
            at("2026-02-01T00:00:00Z"),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn facets_preserve_partial_prices_and_reported_dispatch_coverage() {
    // Given: a fixture store with independently priced/reported sessions.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Facets", Some("app"), Status::Done, "")
        .unwrap();
    let mut dispatch = run("R1", 1);
    dispatch.usage = Some(usage([10, 20, 30, 40], Some(2.0), Some(4.0)));
    let mut reviewer = run("review", 1);
    reviewer.agent = "reviewer".into();
    reviewer.model = Some("review-model".into());
    reviewer.usage = Some(usage([5, 10, 15, 20], None, None));
    dispatch.review_runs.push(reviewer);
    task.runs = vec![dispatch];
    store.save(&task).unwrap();
    // When: the pure collector reads recorded snapshots without repricing.
    let report = collect(&store);
    let row = &report["groups"]["task"][0];
    // Then: partial estimates and all four token categories retain their coverage.
    assert_eq!(report["schema_version"], 1);
    assert_eq!(row["estimated_usd"], 4.0);
    assert!(row["reported_usd"].is_null());
    assert!(row.get("total_cost_usd").is_none());
    assert_eq!(
        row["tokens"],
        json!({"input":15,"output":30,"cache_read":45,"cache_write":60})
    );
    assert_eq!(row["price_coverage"]["priced_tokens"], 100);
    assert_eq!(row["price_coverage"]["known_tokens"], 150);
    assert!((row["price_coverage"]["ratio"].as_f64().unwrap() - 2.0 / 3.0).abs() < 1e-10);
    assert!((row["cache_hit_ratio"].as_f64().unwrap() - 0.375).abs() < 1e-10);
    assert_eq!(row["runs_without_usage"], 0);
    assert_eq!(row["roles"]["implementation"]["reported_usd"], 2.0);
    assert_eq!(row["roles"]["review"]["tokens"]["input"], 5);
    assert_eq!(row["roles"]["plan"], "not_measured");
    assert!(row["estimated_usd_per_landed_task"].is_null());
    assert_eq!(row["rework_share"], 0.0);
}

#[test]
fn unknown_zero_and_unpriced_evidence_remain_distinct() {
    // Given: no usage, known zero, and unpriced nonzero usage in separate groups.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store.create("Coverage", None, Status::Done, "").unwrap();
    for (profile, evidence) in [
        ("missing", None),
        ("zero", Some(usage([0; 4], Some(0.0), Some(0.0)))),
        ("unpriced", Some(usage([12, 0, 0, 0], Some(1.0), None))),
    ] {
        let mut dispatch = run(profile, 1);
        dispatch.agent = profile.into();
        dispatch.usage = evidence;
        task.runs.push(dispatch);
    }
    store.save(&task).unwrap();
    // When: collecting each cohort.
    let report = collect(&store);
    let rows = report["groups"]["profile"].as_array().unwrap();
    let missing = &rows[0];
    let unpriced = &rows[1];
    let zero = &rows[2];
    // Then: missing is null, zero is known, and reported cost does not imply pricing.
    assert_eq!(missing["runs_without_usage"], 1);
    assert!(missing["tokens"].is_null());
    assert!(missing["price_coverage"].is_null());
    assert!(missing["cache_hit_ratio"].is_null());
    assert!(missing["estimated_usd"].is_null());
    assert!(missing["reported_usd"].is_null());
    assert_eq!(
        missing["roles"]["implementation"]["measurement"],
        "not_measured"
    );
    assert_eq!(zero["runs_without_usage"], 0);
    assert_eq!(zero["tokens"]["input"], 0);
    assert_eq!(zero["estimated_usd"], 0.0);
    assert_eq!(zero["reported_usd"], 0.0);
    assert_eq!(zero["price_coverage"]["known_tokens"], 0);
    assert!(zero["price_coverage"]["ratio"].is_null());
    assert!(zero["cache_hit_ratio"].is_null());
    assert!(zero["rework_share"].is_null());
    assert_eq!(unpriced["reported_usd"], 1.0);
    assert!(unpriced["estimated_usd"].is_null());
    assert_eq!(unpriced["price_coverage"]["known_tokens"], 12);
    assert_eq!(unpriced["price_coverage"]["priced_tokens"], 0);
    assert_eq!(unpriced["price_coverage"]["ratio"], 0.0);
}

#[test]
fn reviewer_attempts_stay_in_parent_groups_and_have_separate_roles() {
    // Given: a dispatch with unknown implementation and nested reviewer evidence.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Review", Some("app"), Status::Done, "")
        .unwrap();
    let mut dispatch = run("R1", 1);
    let mut reviewer = run("review", 2);
    reviewer.model = Some("review-model".into());
    reviewer.agent = "review-profile".into();
    reviewer.usage = Some(usage([20, 0, 0, 0], Some(1.0), Some(2.0)));
    let mut retry = reviewer.clone();
    retry.id = "retry".into();
    retry.usage = Some(usage([30, 0, 0, 0], Some(2.0), Some(3.0)));
    reviewer.review_runs.push(retry);
    reviewer.review_runs.push(run("missing-review", 2));
    dispatch.review_runs.push(reviewer);
    task.runs.push(dispatch);
    store.save(&task).unwrap();
    // When: collecting all groupings from the fixture.
    let report = collect(&store);
    // Then: reviewers create no dispatch groups or outcome samples.
    for (by, key) in [
        ("model", "impl-model"),
        ("profile", "implementer"),
        ("day", "2026-01-01"),
    ] {
        let rows = report["groups"][by].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["group"], key);
        assert_eq!(rows[0]["runs"], 1);
        assert_eq!(rows[0]["estimated_usd"], 5.0);
    }
    let row = &report["groups"]["task"][0];
    assert_eq!(row["roles"]["review"]["sessions"], 3);
    assert_eq!(row["roles"]["review"]["sessions_without_usage"], 1);
    assert_eq!(row["roles"]["review"]["measurement"], "partial");
    assert_eq!(row["roles"]["review"]["estimated_usd"], 5.0);
    assert_eq!(row["roles"]["review"]["reported_usd"], 3.0);
    assert_eq!(row["roles"]["implementation"]["sessions_without_usage"], 1);
    assert_eq!(row["runs_without_usage"], 0);
}

#[test]
fn all_groupings_reuse_outcomes_and_count_distinct_landed_tasks() {
    // Given: two approved dispatches on one task plus an unresolved dispatch.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Outcomes", Some("app"), Status::Done, "")
        .unwrap();
    task.kind = Some("logic".into());
    task.size = Some(corral_core::TaskSize::L);
    for (id, day) in [("R1", 1), ("R2", 2), ("R3", 3)] {
        let mut dispatch = run(id, day);
        dispatch.usage = Some(usage([10, 0, 0, 0], Some(1.0), Some(2.0)));
        task.runs.push(dispatch);
    }
    store.save(&task).unwrap();
    let events = vec![
        event(
            &task,
            "R1",
            "changes_requested",
            "2026-01-01T00:10:00Z",
            json!({}),
        ),
        event(
            &task,
            "R1",
            "gate",
            "2026-01-01T00:20:00Z",
            json!({"result":"fail"}),
        ),
        event(
            &task,
            "R1",
            "gate",
            "2026-01-01T00:30:00Z",
            json!({"result":"pass"}),
        ),
        event(&task, "R1", "approved", "2026-01-01T00:40:00Z", json!({})),
        event(&task, "R2", "approved", "2026-01-02T00:40:00Z", json!({})),
    ];
    // When: collecting all eight dimensions.
    let report = serde_json::to_value(
        collect_analytics(
            &store.list().unwrap(),
            &events,
            None,
            at("2026-02-01T00:00:00Z"),
        )
        .unwrap(),
    )
    .unwrap();
    // Then: shared cohort statistics and each requested grouping are present.
    assert_eq!(report["groups"].as_object().unwrap().len(), 8);
    for (by, key) in [
        ("task", task.id.as_str()),
        ("project", "app"),
        ("kind", "logic"),
        ("size", "l"),
        ("profile", "implementer"),
        ("model", "impl-model"),
        ("mode", r#"["impl-model","high","custom"]"#),
    ] {
        let row = &report["groups"][by][0];
        assert_eq!(row["group"], key);
        assert_eq!(row["runs"], 3);
        assert_eq!(row["accepted"], 2);
        assert_eq!(row["landed_tasks"], 1);
        assert_eq!(row["estimated_usd_per_landed_task"], 6.0);
        assert_eq!(row["first_round_accept_rate"], 0.5);
        assert_eq!(row["mean_rounds_to_accept"], 1.5);
        assert_eq!(row["gate_failure_rate"], 0.5);
        assert!(row["rework_share"].is_null());
    }
    assert_eq!(report["groups"]["day"].as_array().unwrap().len(), 3);
}

#[test]
fn timing_attributes_only_estimates_after_first_change_including_reviews() {
    // Given: timed responses before, exactly at, and after a legacy change request.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store.create("Timed", None, Status::Done, "").unwrap();
    let mut dispatch = run("R1", 1);
    let entries = [
        (1.0, "2026-01-01T00:00:01Z"),
        (2.0, "2026-01-01T00:00:02Z"),
        (3.0, "2026-01-01T00:00:03Z"),
    ]
    .map(|(cost, time)| {
        let mut entry = usage([1, 0, 0, 0], None, Some(cost)).entries.remove(0);
        entry.at = Some(at(time));
        entry
    });
    dispatch.usage = Some(Usage::from_entries(entries.into()));
    let mut reviewer = run("review", 1);
    let mut review_usage = usage([1, 0, 0, 0], None, Some(4.0));
    review_usage.entries[0].at = Some(at("2026-01-01T00:00:04Z"));
    reviewer.usage = Some(review_usage);
    dispatch.review_runs.push(reviewer);
    task.runs.push(dispatch);
    store.save(&task).unwrap();
    let events: Vec<Event> = vec![
        serde_json::from_value(json!({
            "task":task.id,"kind":"changes_requested","ts":"2026-01-01T00:00:02Z","detail":{}
        }))
        .unwrap(),
    ];
    // When: collecting recorded timing, without refreshing or reading transcripts.
    let report = serde_json::to_value(
        collect_analytics(
            &store.list().unwrap(),
            &events,
            None,
            at("2026-02-01T00:00:00Z"),
        )
        .unwrap(),
    )
    .unwrap();
    // Then: only the two later responses are rework.
    assert_eq!(report["groups"]["task"][0]["rework_estimated_usd"], 7.0);
    assert_eq!(report["groups"]["task"][0]["rework_share"], 0.7);
}

#[test]
fn cutoff_keeps_earlier_change_requests_and_missing_timing_unavailable() {
    // Given: an earlier change request and an untimed priced retry after the cutoff.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store.create("Retry", None, Status::Done, "").unwrap();
    let mut retry = run("R2", 2);
    retry.usage = Some(usage([10, 0, 0, 0], None, Some(1.0)));
    task.runs = vec![run("R1", 1), retry];
    store.save(&task).unwrap();
    let events = [event(
        &task,
        "R1",
        "changes_requested",
        "2026-01-01T00:01:00Z",
        json!({}),
    )];
    // When: selecting only the retry dispatch.
    let report = serde_json::to_value(
        collect_analytics(
            &store.list().unwrap(),
            &events,
            Some(at("2026-01-02T00:00:00Z")),
            at("2026-02-01T00:00:00Z"),
        )
        .unwrap(),
    )
    .unwrap();
    // Then: old review history prevents a fabricated zero rework share.
    let row = &report["groups"]["task"][0];
    assert_eq!(row["runs"], 1);
    assert!(row["rework_share"].is_null());
    assert!(row["rework_estimated_usd"].is_null());
}

#[test]
fn snapshot_and_dispatch_window_are_inclusive_and_deterministic() {
    // Given: dispatches on each boundary and future outcome events.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store.create("Snapshot", None, Status::Done, "").unwrap();
    task.runs = vec![
        run("old", 1),
        run("start", 2),
        run("end", 3),
        run("future", 4),
    ];
    task.runs[2].outcome = Some(corral_core::RunOutcome::Done);
    task.runs[2].gate = Some(
        serde_json::from_value(json!({
            "result":"fail", "at":"2026-01-04T00:00:00Z", "sha":"abc"
        }))
        .unwrap(),
    );
    store.save(&task).unwrap();
    let events = [
        event(
            &task,
            "start",
            "approved",
            "2026-01-03T00:00:00Z",
            json!({"strategy":"keep_branch"}),
        ),
        event(&task, "end", "approved", "2026-01-04T00:00:00Z", json!({})),
    ];
    let tasks = store.list().unwrap();
    // When: collecting twice with identical evidence and an explicit clock.
    let first = collect_analytics(
        &tasks,
        &events,
        Some(at("2026-01-02T00:00:00Z")),
        at("2026-01-03T00:00:00Z"),
    )
    .unwrap();
    let second = collect_analytics(
        &tasks,
        &events,
        Some(at("2026-01-02T00:00:00Z")),
        at("2026-01-03T00:00:00Z"),
    )
    .unwrap();
    // Then: future dispatch/outcome evidence is excluded and approval is not landing.
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap()
    );
    let row = &first.groups[&StatsBy::Task][0];
    assert_eq!(row.runs, 2);
    assert_eq!(row.accepted, 1);
    assert_eq!(row.gate_failure_rate, None);
    assert_eq!(row.median_wall_seconds, None);
    assert_eq!(row.analytics.landed_tasks, 0);
    assert!(row.analytics.estimated_usd_per_landed_task.is_none());
    assert_eq!(first.groups[&StatsBy::Size][0].group, "unknown");
}

#[test]
fn refreshed_stats_rows_match_the_pure_model_without_legacy_key_changes() {
    // Given: completed known usage that stats can refresh from stored entries.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store.create("Refresh", None, Status::Done, "").unwrap();
    let mut dispatch = run("R1", 1);
    dispatch.model = Some("gpt-6-astra".into());
    dispatch.usage = Some(Usage::from_entries(vec![UsageEntry {
        model: Some("gpt-6-astra".into()),
        input: 1_000_000,
        ..Default::default()
    }]));
    task.runs.push(dispatch);
    store.save(&task).unwrap();
    // When: the public stats adapter refreshes once before pure collection.
    let actual = stats(&store, StatsBy::Task, None).unwrap();
    // The store's own `created` event is real-clock; compare at a cutoff after it.
    let report = serde_json::to_value(
        collect_analytics(
            &store.list().unwrap(),
            &store.events().unwrap(),
            None,
            Timestamp::now(),
        )
        .unwrap(),
    )
    .unwrap();
    // Then: JSON is the same row model with both legacy and new cost fields.
    let row = serde_json::to_value(&actual[0]).unwrap();
    assert_eq!(row, report["groups"]["task"][0]);
    assert_eq!(row["total_estimated_usd"], 10.0);
    assert_eq!(row["estimated_usd"], 10.0);
    assert_eq!(row["usage_runs"], 1);
    assert!(row.get("total_cost_usd").is_none());
}

#[test]
fn missing_reviewer_usage_makes_changed_task_rework_unavailable() {
    // Given: a timed implementation response and an unmeasured reviewer.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Missing review", None, Status::Done, "")
        .unwrap();
    let mut dispatch = run("R1", 1);
    let mut measured = usage([10, 0, 0, 0], None, Some(2.0));
    measured.entries[0].at = Some(at("2026-01-01T00:02:00Z"));
    dispatch.usage = Some(measured);
    dispatch.review_runs.push(run("missing-review", 1));
    task.runs.push(dispatch);
    store.save(&task).unwrap();
    let events = [event(
        &task,
        "R1",
        "changes_requested",
        "2026-01-01T00:01:00Z",
        json!({}),
    )];
    // When: collecting a changed task with partial session coverage.
    let report = collect_analytics(
        &store.list().unwrap(),
        &events,
        None,
        at("2026-02-01T00:00:00Z"),
    )
    .unwrap();
    // Then: the missing session prevents a fabricated full rework share.
    let row = &report.groups[&StatsBy::Task][0];
    assert_eq!(row.analytics.estimated_usd, Some(2.0));
    assert!(row.analytics.rework_estimated_usd.is_none());
    assert!(row.analytics.rework_share.is_none());
}

#[test]
fn planning_sessions_without_usage_are_explicitly_not_measured() {
    // Given: a persisted planning session alongside an implementation dispatch.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store.create("Planning", None, Status::Done, "").unwrap();
    task.runs.push(run("R1", 2));
    task.plan_runs.push(
        serde_json::from_value(json!({
            "id":"P1", "profile":"planner", "harness":"omo", "model":"planner-model",
            "spec_revision":"abc", "started":"2026-01-01T00:00:00Z",
            "output_dir":"/unused", "rounds":0, "state":"approved"
        }))
        .unwrap(),
    );
    store.save(&task).unwrap();
    // When: collecting from stored planning records which have no usage contract.
    let report = collect(&store);
    // Then: planning is neither fabricated zero spend nor another dispatch.
    let row = &report["groups"]["task"][0];
    assert_eq!(row["roles"]["plan"], "not_measured");
    assert_eq!(row["runs"], 1);
    assert!(row["estimated_usd"].is_null());
}

#[test]
fn future_discard_does_not_resolve_review_before_snapshot() {
    // Given: a stored discard and its event both occur after the explicit snapshot.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Future discard", None, Status::Dropped, "")
        .unwrap();
    let mut dispatch = run("R1", 1);
    dispatch.outcome = Some(corral_core::RunOutcome::Discarded);
    dispatch.ended = Some(at("2026-01-04T00:00:00Z"));
    task.runs.push(dispatch);
    store.save(&task).unwrap();
    let events = [event(
        &task,
        "R1",
        "discarded",
        "2026-01-04T00:00:00Z",
        json!({}),
    )];
    // When: collecting the cohort at January 3.
    let report = collect_analytics(
        &store.list().unwrap(),
        &events,
        None,
        at("2026-01-03T00:00:00Z"),
    )
    .unwrap();
    // Then: the future discard contributes neither a discard nor a resolved first review.
    for rows in report.groups.values() {
        let row = &rows[0];
        assert_eq!(row.discards, 0, "group {}", row.group);
        assert_eq!(row.first_round_accept_rate, None, "group {}", row.group);
    }
}

#[test]
fn discard_snapshot_keeps_inclusive_events_and_legacy_fallbacks() {
    // Given: boundary, past, undated, and dated-event-authoritative discard evidence.
    for (end, event_time) in [
        (Some("2026-01-03T00:00:00Z"), None),
        (Some("2026-01-03T00:00:00Z"), Some("2026-01-03T00:00:00Z")),
        (Some("2026-01-02T00:00:00Z"), None),
        (None, None),
        (Some("2026-01-04T00:00:00Z"), Some("2026-01-03T00:00:00Z")),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path()).unwrap();
        let mut task = store
            .create("Known discard", None, Status::Dropped, "")
            .unwrap();
        let mut dispatch = run("R1", 1);
        dispatch.outcome = Some(corral_core::RunOutcome::Discarded);
        dispatch.ended = end.map(at);
        task.runs.push(dispatch);
        store.save(&task).unwrap();
        let events: Vec<_> = event_time
            .map(|time| event(&task, "R1", "discarded", time, json!({})))
            .into_iter()
            .collect();
        // When: collecting exactly at the inclusive January 3 boundary.
        let report = collect_analytics(
            &store.list().unwrap(),
            &events,
            None,
            at("2026-01-03T00:00:00Z"),
        )
        .unwrap();
        // Then: known discards resolve one first-review sample, never two.
        let row = &report.groups[&StatsBy::Task][0];
        assert_eq!(row.discards, 1, "end {end:?}, event {event_time:?}");
        assert_eq!(
            row.first_round_accept_rate,
            Some(0.0),
            "end {end:?}, event {event_time:?}"
        );
    }
}

#[test]
fn future_discard_does_not_dilute_known_first_round_acceptance() {
    // Given: a first-round approval and a future stored discard in the same cohort.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Review denominator", None, Status::Done, "")
        .unwrap();
    let mut future = run("R2", 2);
    future.outcome = Some(corral_core::RunOutcome::Discarded);
    future.ended = Some(at("2026-01-04T00:00:00Z"));
    task.runs = vec![run("R1", 1), future];
    store.save(&task).unwrap();
    let events = [event(
        &task,
        "R1",
        "approved",
        "2026-01-01T00:10:00Z",
        json!({}),
    )];
    // When: collecting before the discard ends, without a discard event.
    let report = collect_analytics(
        &store.list().unwrap(),
        &events,
        None,
        at("2026-01-03T00:00:00Z"),
    )
    .unwrap();
    // Then: only the approval enters the resolved-review denominator.
    let row = &report.groups[&StatsBy::Task][0];
    assert_eq!(row.runs, 2);
    assert_eq!(row.accepted, 1);
    assert_eq!(row.first_round_accept_rate, Some(1.0));
    assert_eq!(row.discards, 0);
}

#[test]
fn phase_totals_group_by_project_kind_size_and_model() {
    // Given: two projects whose gates take ten and two minutes, plus one lost gate.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut events = Vec::new();
    for (project, gate_minutes) in [("slow", 10), ("fast", 2)] {
        let mut task = store
            .create(project, Some(project), Status::Done, "")
            .unwrap();
        task.kind = Some("logic".into());
        task.size = Some(corral_core::TaskSize::M);
        task.runs.push(run("R1", 1));
        store.save(&task).unwrap();
        let end = format!("2026-01-01T00:{:02}:00Z", 20 + gate_minutes);
        for (kind, time, detail) in [
            ("dispatched", "2026-01-01T00:00:00Z", json!({"run":"R1"})),
            ("done", "2026-01-01T00:15:00Z", json!({"run":"R1"})),
            ("gate_started", "2026-01-01T00:16:00Z", json!({"run":"R1"})),
            (
                "review_released",
                "2026-01-01T00:18:00Z",
                json!({"run":"R1","kind":"gate"}),
            ),
            ("gate_started", "2026-01-01T00:20:00Z", json!({"run":"R1"})),
            ("gate", end.as_str(), json!({"run":"R1","result":"pass"})),
        ] {
            events.push(
                serde_json::from_value::<Event>(json!({
                    "task":task.id, "kind":kind, "ts":time, "detail":detail
                }))
                .unwrap(),
            );
        }
    }
    // When: collecting every grouping.
    let report = serde_json::to_value(
        collect_analytics(
            &store.list().unwrap(),
            &events,
            None,
            at("2026-02-01T00:00:00Z"),
        )
        .unwrap(),
    )
    .unwrap();
    // Then: projects separate their gate time, and the lost attempt is only counted.
    let projects = report["groups"]["project"].as_array().unwrap();
    let gate = |group: &str| {
        projects.iter().find(|row| row["group"] == group).unwrap()["phases"]["gate"].clone()
    };
    assert_eq!(
        gate("slow"),
        json!({"complete":1,"incomplete":1,"total_seconds":600.0,"median_seconds":600.0})
    );
    assert_eq!(gate("fast")["total_seconds"], 120.0);
    assert_eq!(projects[0]["phases"]["active"]["total_seconds"], 900.0);
    for by in ["kind", "size", "model"] {
        let row = &report["groups"][by][0];
        assert_eq!(row["phases"]["gate"]["complete"], 2, "{by}");
        assert_eq!(row["phases"]["gate"]["incomplete"], 2, "{by}");
        assert_eq!(row["phases"]["gate"]["median_seconds"], 360.0, "{by}");
        assert_eq!(row["phases"]["cycle"]["incomplete"], 2, "{by}");
    }
}
