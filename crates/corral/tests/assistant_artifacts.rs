use std::path::Path;
use std::process::{Command, Output, Stdio};

fn run(root: &Path, assistant: Option<&str>, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
    for name in [
        "CORRAL_ASSISTANT",
        "CORRAL_PM",
        "CORRAL_GATE_ATTEMPT",
        "CORRAL_CANDIDATE_HEAD",
        "CORRAL_INTEGRATION_BASE",
        "HERDR_PANE_ID",
        "HERDR_WORKSPACE_ID",
        "HERDR_TAB_ID",
    ] {
        cmd.env_remove(name);
    }
    if let Some(session) = assistant {
        cmd.env("CORRAL_ASSISTANT", session);
    }
    cmd.env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn ok(root: &Path, assistant: Option<&str>, args: &[&str]) -> String {
    let out = run(root, assistant, args);
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn session_artifact_cli_and_ownership_table() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, None, &["init"]);
    for _ in 0..2 {
        ok(root, None, &["assistant", "create"]);
    }
    ok(root, None, &["new", "ordinary task"]);
    std::fs::write(
        root.join("report.md"),
        "# Session finding\nunique_session_needle\n",
    )
    .unwrap();
    ok(
        root,
        Some("A-1"),
        &[
            "artifact",
            "add",
            "--session",
            "A-1",
            "report.md",
            "--kind",
            "report",
        ],
    );
    let list = ok(
        root,
        None,
        &["artifact", "list", "--session", "A-1", "--json"],
    );
    let rows: serde_json::Value = serde_json::from_str(&list).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["session"], "A-1");
    assert!(rows[0]["task"].is_null());
    assert_eq!(rows[0]["tier"], "keep");
    let snapshot = rows[0]["artifact"]["path"].as_str().unwrap();
    assert!(snapshot.starts_with("artifacts/assistant/A-1/"));
    assert!(root.join(".corral").join(snapshot).is_file());
    assert_eq!(
        ok(root, None, &["artifact", "show", "A-1:report.md", "--raw"]),
        "# Session finding\nunique_session_needle\n"
    );
    ok(root, None, &["pin", "A-1:report.md", "--follow-latest"]);
    let pins: serde_json::Value =
        serde_json::from_str(&ok(root, None, &["pins", "--json"])).unwrap();
    assert_eq!(pins[0]["state"], "ok");
    assert!(pins[0]["ref"].as_str().unwrap().starts_with("A-1:"));
    assert!(ok(root, None, &["artifact", "show", "1", "--raw"]).contains("unique_session_needle"));
    let hits = ok(
        root,
        None,
        &[
            "search",
            "unique_session_needle",
            "--in",
            "artifacts",
            "--json",
        ],
    );
    assert!(hits.contains("A-1:"), "{hits}");
    ok(
        root,
        None,
        &["artifact", "set-tier", "A-1:report.md", "normal"],
    );
    ok(root, None, &["artifact", "gc", "--dry-run", "--json"]);
    let before_session = std::fs::read(root.join(".corral/assistants/A-2.md")).unwrap();
    let store = corral_core::Store::open(root).unwrap();
    let task_before = store.get("T-001").unwrap();
    // Every add destination has an explicit allow/refuse case. Refusals leave both owners unchanged.
    for (target, allowed) in [
        (vec!["--session", "A-1", "report.md"], true),
        (vec!["--session", "A-2", "report.md"], false),
        (vec!["T-001", "report.md"], false),
    ] {
        let mut args = vec!["artifact", "add"];
        args.extend(target);
        let out = run(root, Some("A-1"), &args);
        assert_eq!(out.status.success(), allowed, "{args:?}: {out:?}");
        if !allowed {
            assert!(String::from_utf8_lossy(&out.stderr).contains("assistant_refused"));
        }
    }
    assert_eq!(
        before_session,
        std::fs::read(root.join(".corral/assistants/A-2.md")).unwrap()
    );
    assert_eq!(task_before, store.get("T-001").unwrap());
    ok(root, None, &["unpin", "A-1:report.md"]);
}
