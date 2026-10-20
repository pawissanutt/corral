use std::process::{Command, Output};

fn invoke(root: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_corral"))
        .env("CORRAL_ROOT", root)
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn stats_outputs_table_and_json_without_contacting_herdr() {
    // Given a stored run with a recorded approval.
    let tmp = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Work", None, corral_core::Status::Done, "")
        .unwrap();
    task.runs.push(
        serde_json::from_value(serde_json::json!({
            "id":"R1", "agent":"fast", "model":"model-a", "worktree":"/unused",
            "branch":"test", "base":"main", "base_sha":"abc", "started":"2026-01-01T00:00:00Z"
        }))
        .unwrap(),
    );
    store.save(&task).unwrap();
    store
        .log_event(Some(&task.id), "approved", serde_json::json!({"run":"R1"}))
        .unwrap();
    // When the real binary renders both formats.
    let table = invoke(tmp.path(), &["stats"]);
    let json = invoke(tmp.path(), &["stats", "--by", "model", "--json"]);
    // Then the table has numeric evidence and JSON preserves structured values.
    assert!(table.status.success(), "{:?}", table);
    let text = String::from_utf8(table.stdout).unwrap();
    assert_eq!(text.lines().count(), 2);
    let cells: Vec<_> = text.lines().nth(1).unwrap().split_whitespace().collect();
    assert_eq!(cells, ["fast", "1", "100.0%", "1.0", "-", "0", "-", "0"]);
    assert!(json.status.success(), "{:?}", json);
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value[0]["group"], "model-a");
    assert_eq!(value[0]["runs"], 1);
    assert_eq!(value[0]["first_round_accept_rate"], 1.0);
    assert_eq!(value[0]["schema_version"], 1);
    assert_eq!(value[0]["runs_without_usage"], 1);
    assert!(value[0]["tokens"].is_null());
    assert_eq!(value[0]["roles"]["plan"], "not_measured");
    assert!(value[0].get("cost").is_none());
    for field in [
        "median_tokens_per_run",
        "total_tokens",
        "usage_runs",
        "median_cost_usd_per_run",
        "total_cost_usd",
        "cost_runs",
    ] {
        assert!(value[0].get(field).is_none(), "{field}");
    }
    assert!(!text.contains("TOKENS"));
    assert!(!text.contains("USD"));
    assert!(
        invoke(tmp.path(), &["stats", "--since", "30d", "--json"])
            .status
            .success()
    );
    assert!(
        !invoke(tmp.path(), &["stats", "--since", "bad"])
            .status
            .success()
    );
    assert!(
        !invoke(tmp.path(), &["stats", "--by", "invalid"])
            .status
            .success()
    );
}

#[test]
fn stats_table_adds_only_available_usage_columns_and_marks_missing_rows() {
    // Given known zero tokens with optional cost, alongside a missing-usage group.
    for cost in [None, Some(0.0)] {
        let tmp = tempfile::tempdir().unwrap();
        let store = corral_core::Store::init(tmp.path()).unwrap();
        let mut task = store
            .create("Usage", None, corral_core::Status::Done, "")
            .unwrap();
        for (agent, usage) in [
            (
                "known",
                serde_json::json!({"input":0,"output":0,"cache_read":0,"cache_write":0,"cost_usd":cost}),
            ),
            ("missing", serde_json::Value::Null),
        ] {
            task.runs.push(
                serde_json::from_value(serde_json::json!({
                    "id": agent, "agent": agent, "worktree":"/unused", "branch":"test",
                    "base":"main", "base_sha":"abc", "started":"2026-01-01T00:00:00Z",
                "usage": usage, "ended":"2026-01-01T00:01:00Z"
                }))
                .unwrap(),
            );
        }
        store.save(&task).unwrap();
        // When the real binary renders a table.
        let output = invoke(tmp.path(), &["stats"]);
        // Then available zero values display numerically and absent rows use dashes.
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert!(lines[0].contains("MEDIAN TOKENS"));
        assert!(lines[0].contains("TOTAL TOKENS"));
        assert_eq!(lines[0].contains("MEDIAN USD"), cost.is_some());
        assert_eq!(lines[0].contains("TOTAL USD"), cost.is_some());
        let known: Vec<_> = lines[1].split_whitespace().collect();
        let missing: Vec<_> = lines[2].split_whitespace().collect();
        assert_eq!(&known[8..10], ["0.0", "0"]);
        assert_eq!(&missing[8..10], ["-", "-"]);
        if cost.is_some() {
            assert_eq!(&known[10..], ["0.0000", "0.0000"]);
            assert_eq!(&missing[10..], ["-", "-"]);
        } else {
            assert_eq!(known.len(), 10);
            assert_eq!(missing.len(), 10);
        }
    }
}

#[test]
fn show_and_task_stats_refresh_and_include_review_costs() {
    use serde_json::json;

    // Given persisted legacy usage with a priced reviewer and an unpriced retry.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = corral_core::Store::init(tmp.path()).unwrap();
    store.config.pricing.insert(
        "priced".into(),
        serde_json::from_value(json!({"input":10000.0,"output":0.0})).unwrap(),
    );
    store.save_config().unwrap();
    let mut task = store
        .create("Costs", None, corral_core::Status::Done, "")
        .unwrap();
    let mut first = json!({
        "id":"R1","agent":"test","model":"priced","worktree":"/unused",
        "branch":"test","base":"main","base_sha":"abc",
        "started":"2026-01-01T00:00:00Z","ended":"2026-01-01T00:01:00Z",
        "usage":{"input":10,"output":0,"cache_read":0,"cache_write":0,"cost_usd":0.1}
    });
    let mut reviewer = first.clone();
    reviewer["id"] = json!("review-1");
    reviewer["usage"]["input"] = json!(20);
    reviewer["usage"]["cost_usd"] = json!(0.2);
    first["review_runs"] = json!([reviewer]);
    let mut retry = first.clone();
    retry["id"] = json!("R2");
    retry["model"] = serde_json::Value::Null;
    retry["review_runs"] = json!([]);
    retry["usage"] = json!({"input":5,"output":0,"cache_read":0,"cache_write":0});
    task.runs = vec![
        serde_json::from_value(first).unwrap(),
        serde_json::from_value(retry).unwrap(),
    ];
    store.save(&task).unwrap();
    let mut unrelated = store
        .create("Unrelated", None, corral_core::Status::Done, "")
        .unwrap();
    unrelated.runs = vec![task.runs[0].clone()];
    store.save(&unrelated).unwrap();

    // When show refreshes the target before producing JSON, then stats groups by task.
    let show = invoke(tmp.path(), &["show", &task.id, "--json"]);
    assert!(show.status.success(), "{show:?}");
    let value: serde_json::Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(value["total_usage"]["input"], 35);
    assert!((value["total_usage"]["estimated_usd"].as_f64().unwrap() - 0.3).abs() < 1e-9);
    assert_eq!(value["total_usage"]["unpriced_tokens"], 5);
    assert_eq!(value["total_usage"]["cost_basis"], "mixed");
    assert_eq!(
        value["runs"][0]["review_runs"][0]["usage"]["entries"][0]["input"],
        20
    );
    assert!(
        store.get(&unrelated.id).unwrap().runs[0]
            .usage
            .as_ref()
            .unwrap()
            .entries
            .is_empty()
    );

    let text = invoke(tmp.path(), &["show", &task.id]);
    assert!(text.status.success(), "{text:?}");
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("reviewer run review-1"), "{text}");
    assert_eq!(
        text.lines()
            .filter(|line| line.trim_start().starts_with("cost:"))
            .count(),
        3
    );
    assert!(
        text.contains(
            &store
                .get(&task.id)
                .unwrap()
                .total_usage()
                .unwrap()
                .summary()
        ),
        "{text}"
    );

    let output = invoke(tmp.path(), &["stats", "--by", "task", "--json"]);
    assert!(output.status.success(), "{output:?}");
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["group"] == task.id)
        .unwrap();
    assert_eq!(row["runs"], 2);
    assert_eq!(row["total_tokens"], 35);
    assert_eq!(row["unpriced_tokens"], 5);
    assert_eq!(row["cost_basis"], "mixed");
    assert_eq!(row["tokens"]["input"], 35);
    assert_eq!(row["price_coverage"]["priced_tokens"], 30);
    assert_eq!(row["price_coverage"]["known_tokens"], 35);
    assert_eq!(row["roles"]["review"]["sessions"], 1);
    assert_eq!(row["roles"]["review"]["tokens"]["input"], 20);
    assert!((row["roles"]["review"]["estimated_usd"].as_f64().unwrap() - 0.2).abs() < 1e-9);
    assert!((row["total_cost_usd"].as_f64().unwrap() - 0.3).abs() < 1e-9);
    assert!((row["total_estimated_usd"].as_f64().unwrap() - 0.3).abs() < 1e-9);
    assert!((row["median_estimated_usd_per_run"].as_f64().unwrap() - 0.3).abs() < 1e-9);
    let table = invoke(tmp.path(), &["stats", "--by", "task"]);
    assert!(table.status.success(), "{table:?}");
    let table = String::from_utf8(table.stdout).unwrap();
    assert!(
        table.contains("MEDIAN EST USD") && table.contains("UNPRICED TOKENS"),
        "{table}"
    );
}

#[test]
fn stats_compares_observed_lesson_cohorts_in_text_and_json() {
    use serde_json::json;

    // Given all three exposure cohorts sharing a profile.
    let tmp = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Lessons", Some("app"), corral_core::Status::Done, "")
        .unwrap();
    for id in ["exposed", "unexposed", "unknown"] {
        task.runs.push(
            serde_json::from_value(json!({
                "id":id, "agent":"fast", "worktree":"/unused", "branch":"test",
                "base":"main", "base_sha":"abc", "started":"2026-01-01T00:00:00Z"
            }))
            .unwrap(),
        );
    }
    store.save(&task).unwrap();
    let lesson: corral_core::Lesson = serde_json::from_value(json!({
        "id":"L1", "rule":"Run gates", "why":"Check before accepting",
        "confidence":0.8, "last_seen":"2025-12-30T00:00:00Z"
    }))
    .unwrap();
    for (id, lessons) in [("exposed", json!([lesson])), ("unexposed", json!([]))] {
        store
            .log_event(
                Some(&task.id),
                "lessons_applied",
                json!({"run":id,"lessons":lessons,"project":"app"}),
            )
            .unwrap();
    }
    for kind in ["changes_requested", "changes_requested", "approved"] {
        store
            .log_event(Some(&task.id), kind, json!({"run":"exposed"}))
            .unwrap();
    }
    for result in ["fail", "pass"] {
        store
            .log_event(
                Some(&task.id),
                "gate",
                json!({"run":"exposed","result":result}),
            )
            .unwrap();
    }

    // When the real binary renders stats in both formats.
    let table = invoke(tmp.path(), &["stats"]);
    let output = invoke(tmp.path(), &["stats", "--json"]);

    // Then the compact comparison exposes counts and denominators without a delta.
    assert!(table.status.success(), "{table:?}");
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(table.stdout).unwrap();
    assert_eq!(text.lines().count(), 3);
    let (_, comparison) = text.lines().nth(2).unwrap().split_once(':').unwrap();
    assert_eq!(
        comparison.split_whitespace().collect::<Vec<_>>(),
        [
            "exposed",
            "1/1",
            "3.0",
            "50.0%(1/2)",
            "unexposed",
            "1/0",
            "-",
            "-(0/0)",
            "unknown",
            "1/0",
            "-",
            "-(0/0)"
        ]
    );
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let cohorts = &rows[0]["lesson_effectiveness"];
    assert_eq!(cohorts["exposed"]["runs"], 1);
    assert_eq!(cohorts["exposed"]["accepted"], 1);
    assert_eq!(cohorts["exposed"]["mean_rounds_to_accept"], 3.0);
    assert_eq!(cohorts["exposed"]["gate_failure_rate"], 0.5);
    assert_eq!(cohorts["exposed"]["gate_attempts"], 2);
    assert_eq!(cohorts["exposed"]["gate_failures"], 1);
    assert_eq!(cohorts["exposed"]["changes_requested"], 2);
    assert_eq!(cohorts["exposed"]["rounds_to_accept"], 3);
    assert_eq!(cohorts["unexposed"]["runs"], 1);
    assert_eq!(cohorts["unknown"]["runs"], 1);
    assert!(cohorts["unexposed"]["mean_rounds_to_accept"].is_null());
    assert!(cohorts["unknown"]["gate_failure_rate"].is_null());
}

#[test]
fn stats_new_groupings_use_dispatch_metadata_and_utc_days() {
    // Given: dispatch mode metadata differing only in effort and harness.
    let tmp = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Groups", None, corral_core::Status::Done, "")
        .unwrap();
    task.size = Some(corral_core::TaskSize::L);
    for (id, harness, effort, mode) in [
        ("R1", "omo", "high", "interactive"),
        ("R2", "omo", "low", "interactive"),
        ("R3", "omp", "high", "headless"),
        ("R4", "omo", "high", "headless"),
    ] {
        task.runs.push(
            serde_json::from_value(serde_json::json!({
                "id":id, "agent":"fixture", "model":"model-a", "harness":harness,
                "effort":effort, "mode":mode, "worktree":"/unused", "branch":"test",
                "base":"main", "base_sha":"abc",
                "started":"2026-01-01T23:00:00-02:00", "ended":"2026-01-02T02:00:00Z"
            }))
            .unwrap(),
        );
    }
    store.save(&task).unwrap();
    // When: the real binary renders the opt-in grouping flags.
    let outputs =
        ["size", "mode", "day"].map(|by| invoke(tmp.path(), &["stats", "--by", by, "--json"]));
    // Then: tuple identity includes effort/harness but not interactive/headless.
    for output in &outputs {
        assert!(output.status.success(), "{output:?}");
    }
    let size: serde_json::Value = serde_json::from_slice(&outputs[0].stdout).unwrap();
    assert_eq!(size[0]["group"], "l");
    assert_eq!(size[0]["runs"], 4);
    let modes: serde_json::Value = serde_json::from_slice(&outputs[1].stdout).unwrap();
    assert_eq!(modes.as_array().unwrap().len(), 3);
    let high = modes
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["group"] == r#"["model-a","high","omo"]"#)
        .unwrap();
    assert_eq!(high["runs"], 2);
    let days: serde_json::Value = serde_json::from_slice(&outputs[2].stdout).unwrap();
    assert_eq!(days[0]["group"], "2026-01-02");
}
