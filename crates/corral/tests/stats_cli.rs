use std::process::{Command, Output};

fn invoke(root: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_corral"))
        .env_remove("CORRAL_ROOT")
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
    let cells: Vec<_> = text.lines().nth(1).unwrap().split_whitespace().collect();
    assert_eq!(cells, ["fast", "1", "100.0%", "1.0", "-", "0", "-", "0"]);
    assert!(json.status.success(), "{:?}", json);
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value[0]["group"], "model-a");
    assert_eq!(value[0]["runs"], 1);
    assert_eq!(value[0]["first_round_accept_rate"], 1.0);
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
