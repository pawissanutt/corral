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
