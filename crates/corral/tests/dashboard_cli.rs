use corral_core::{Status, Store};
use std::{
    path::Path,
    process::{Command, Output, Stdio},
};

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_corral"))
        .current_dir(root)
        .env("CORRAL_ROOT", root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_SESSION")
        .env_remove("CORRAL_HERDR_SESSION")
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn ok(root: &Path, args: &[&str]) -> serde_json::Value {
    let output = cli(root, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn dashboard_cli_authorization_show_pause_resume_remove() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("app")).unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create(
            "Report",
            Some("app"),
            Status::Backlog,
            "Regenerate report.html",
        )
        .unwrap();
    let set = [
        "dashboard",
        "set",
        &task.id,
        "--interval-secs",
        "60",
        "--next-due",
        "2026-10-16T12:00:00Z",
    ];
    assert!(!cli(tmp.path(), &set).status.success());
    assert!(store.get(&task.id).unwrap().dashboard.is_none());
    let authorized: Vec<_> = set.into_iter().chain(["--authorize-dispatch"]).collect();
    let value = ok(tmp.path(), &authorized);
    assert_eq!(value["interval_secs"], 60);
    assert_eq!(value["next_due"], "2026-10-16T12:00:00Z");
    assert_eq!(value["paused"], false);
    assert_eq!(ok(tmp.path(), &["dashboard", "show", &task.id]), value);
    assert_eq!(
        ok(tmp.path(), &["dashboard", "pause", &task.id])["paused"],
        true
    );
    assert_eq!(ok(tmp.path(), &authorized)["paused"], false);
    assert_eq!(
        ok(tmp.path(), &["dashboard", "remove", &task.id]),
        serde_json::Value::Null
    );
    assert!(store.get(&task.id).unwrap().dashboard.is_none());
}
