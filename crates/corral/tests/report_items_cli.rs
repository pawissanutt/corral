use std::process::Command;

#[test]
fn by_item_json_without_a_pm_returns_zero_usage_without_herdr() {
    let tmp = tempfile::tempdir().unwrap();
    corral_core::Store::init(tmp.path()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_corral"))
        .env("CORRAL_ROOT", tmp.path())
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .arg("--root")
        .arg(tmp.path())
        .args(["report", "--by-item", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["coverage"]["total_requests"], 0);
    assert_eq!(value["coverage"]["attributed_requests"], 0);
    assert_eq!(value["coverage"]["legacy"], 0);
    assert_eq!(value["coverage"]["dropped"], 0);
    for field in [
        "input",
        "cache_read",
        "cache_write",
        "output",
        "unpriced_tokens",
    ] {
        assert_eq!(value["total"][field], 0, "{field}");
    }
    assert!(value["total"]["estimated_usd"].is_null());
    let rows = value["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["class"], "overhead");
    assert_eq!(rows[1]["class"], "unallocated");
    for row in rows {
        assert_eq!(row["requests"], 0);
        assert_eq!(row["input"], 0);
        assert_eq!(row["output"], 0);
    }
    assert!(value.get("filtered_project").is_none());
}
