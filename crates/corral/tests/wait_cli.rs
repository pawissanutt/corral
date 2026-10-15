use std::process::{Command, Output, Stdio};

// Nonzero CLI timeouts below are hang guards for exact retained events, not timing assertions.
fn invoke(root: &std::path::Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("CORRAL_") || name.starts_with("HERDR_") {
            command.env_remove(key);
        }
    }
    command
        .env("CORRAL_ROOT", root)
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .arg("--root")
        .arg(root)
        .arg("wait")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn configuration_events_are_readable_by_harnesses_in_a_new_process() {
    let dir = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(dir.path()).unwrap();
    store
        .edit_config(|doc| doc.set_str("ui.mouse", "false"))
        .unwrap();
    let output = invoke(
        dir.path(),
        &[
            "--for",
            "config_changed",
            "--after-offset",
            "0",
            "--timeout",
            "120s",
            "--json",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let event: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(event["kind"], "config_changed");
    assert_eq!(event["detail"]["pm_relevant"], false);
    assert_eq!(event["detail"]["changes"][0]["key"], "ui.mouse");
}

#[test]
fn timeout_exits_124_without_output() {
    let dir = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(dir.path()).unwrap();
    store
        .log_event(Some("T-001"), "done", serde_json::json!({}))
        .unwrap();
    let output = invoke(dir.path(), &["--timeout", "0ms"]);
    assert_eq!(output.status.code(), Some(124));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn after_offset_is_gapless_across_two_processes() {
    let dir = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(dir.path()).unwrap();
    store
        .log_event(Some("T-001"), "created", serde_json::json!({}))
        .unwrap();
    store
        .log_event(
            Some("T-001"),
            "done",
            serde_json::json!({"run":"R1","added":3,"removed":1}),
        )
        .unwrap();
    store
        .log_event(
            Some("T-002"),
            "autopilot",
            serde_json::json!({"to":"escalated"}),
        )
        .unwrap();
    let first = invoke(
        dir.path(),
        &[
            "--after-offset",
            "0",
            "--json",
            "--print-cursor",
            "--timeout",
            "120s",
        ],
    );
    assert!(first.status.success(), "{first:?}");
    assert_eq!(String::from_utf8_lossy(&first.stdout).lines().count(), 1);
    let value: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(value["kind"], "done");
    let offset = value["after_offset"].as_u64().unwrap().to_string();
    let second = invoke(
        dir.path(),
        &[
            "--after-offset",
            &offset,
            "--for",
            "escalated",
            "--timeout",
            "120s",
        ],
    );
    assert!(second.status.success(), "{second:?}");
    assert_eq!(
        String::from_utf8(second.stdout).unwrap(),
        "T-002 escalated\n"
    );
}

#[test]
fn since_replays_history_and_normalizes_task_id_with_project_filter() {
    let dir = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(dir.path()).unwrap();
    let task = store
        .create("target", Some("app"), corral_core::Status::Review, "")
        .unwrap();
    store
        .log_event(
            Some(&task.id),
            "autopilot",
            serde_json::json!({"to":"blocked"}),
        )
        .unwrap();
    let output = invoke(
        dir.path(),
        &[
            "--since",
            "2000-01-01T00:00:00Z",
            "--task",
            "1",
            "--project",
            "app",
            "--for",
            "blocked",
            "--json",
            "--timeout",
            "120s",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["task"], task.id);
    assert_eq!(value["detail"]["to"], "blocked");
    assert!(value.get("cursor").is_none());
}
