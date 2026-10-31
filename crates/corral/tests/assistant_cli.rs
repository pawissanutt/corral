use corral_core::Store;
use std::process::{Command, Output, Stdio};

fn invoke(root: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_corral"))
        .arg("--root")
        .arg(root)
        .args(args)
        .env_remove("CORRAL_PM")
        .env_remove("CORRAL_ASSISTANT")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn ok(root: &std::path::Path, args: &[&str]) -> String {
    let output = invoke(root, args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn two_sessions_create_list_show_summary_end_and_stats() {
    let tmp = tempfile::tempdir().unwrap();
    Store::init(tmp.path()).unwrap();
    assert_eq!(
        ok(tmp.path(), &["assistant", "create", "--pane", "pane-1"]),
        "A-1\n"
    );
    let created: serde_json::Value = serde_json::from_str(&ok(
        tmp.path(),
        &[
            "assistant",
            "create",
            "--focus",
            "T-42",
            "--pane",
            "pane-2",
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(created["id"], "A-2");
    // All lifecycle read/write command variants have an explicit CLI case here.
    for (args, expected) in [
        (
            vec!["assistant", "list"],
            "A-1 open workspace pane-1 unknown",
        ),
        (vec!["assistant", "show", "A-2"], "focus=T-42"),
        (vec!["stats"], "assistant    2 unknown"),
    ] {
        assert!(ok(tmp.path(), &args).contains(expected));
    }
    let summary = tmp.path().join("summary.md");
    std::fs::write(&summary, "# Findings\n\nUseful notes.\n").unwrap();
    ok(
        tmp.path(),
        &[
            "assistant",
            "summary",
            "A-1",
            "--file",
            summary.to_str().unwrap(),
        ],
    );
    assert!(ok(tmp.path(), &["assistant", "show", "A-1"]).contains("Useful notes."));
    for id in ["A-1", "A-2"] {
        ok(tmp.path(), &["assistant", "end", id]);
        let shown: serde_json::Value =
            serde_json::from_str(&ok(tmp.path(), &["assistant", "show", id, "--json"])).unwrap();
        assert!(shown["session"]["ended"].is_string());
        assert_eq!(shown["container"]["state"]["binding"]["role"], "assistant");
        assert_eq!(shown["container"]["state"]["binding"]["closing"], true);
    }
    let listed: serde_json::Value =
        serde_json::from_str(&ok(tmp.path(), &["assistant", "list", "--json"])).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 2);
    let stats: serde_json::Value =
        serde_json::from_str(&ok(tmp.path(), &["stats", "--json"])).unwrap();
    let role = stats
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["role"] == "assistant")
        .unwrap();
    assert_eq!(role["sessions"][0]["session"], "A-1");
    assert_eq!(role["sessions"][1]["session"], "A-2");
    assert_eq!(role["cost"], "unknown");
    assert!(ok(tmp.path(), &["stats"]).contains("A-2 cost: unknown"));
    assert!(Store::open(tmp.path()).unwrap().list().unwrap().is_empty());
}
