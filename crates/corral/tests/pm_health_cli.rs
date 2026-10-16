//! PM health readbacks must work without a terminal server or host configuration.
use std::path::Path;
use std::process::{Command, Stdio};

use corral_core::{Status, Store};
use serde_json::{Value, json};

fn brief(root: &Path) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_corral"))
        .env_clear()
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join(".config"))
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .env("HERDR_BIN_PATH", root.join("no-engine"))
        .arg("--root")
        .arg(root)
        .args(["pm", "brief", "--json"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn add_run(store: &Store, status: Status, fields: Value) -> String {
    let task = store.create("Health fixture", None, status, "").unwrap();
    store
        .update(&task.id, |task| {
            let mut run = json!({
                "id":"R1", "agent":"generic", "worktree":store.root().join("missing"),
                "branch":"corral/fixture", "base":"main", "base_sha":"abc",
                "started":"2026-10-15T00:00:00Z"
            });
            run.as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            task.runs.push(serde_json::from_value(run)?);
            Ok(())
        })
        .unwrap();
    task.id
}

#[test]
fn pm_brief_reports_unbound_legacy_ownership_until_explicit_binding() {
    // Given a legacy pane reference with no saved launch socket.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    add_run(
        &store,
        Status::Done,
        json!({"herdr":{"workspace_id":"w1","pane_id":"w1:p1"}}),
    );
    let events = std::fs::read(store.dir().join("events.jsonl")).unwrap();
    // When querying the real CLI without an engine.
    let health = brief(temp.path())["health"].clone();
    // Then reconciliation is explicitly unavailable and the readback wrote nothing.
    assert_eq!(health["sync_owned"], false);
    assert_eq!(
        events,
        std::fs::read(store.dir().join("events.jsonl")).unwrap()
    );
    let socket = temp
        .path()
        .join(".config/herdr/sessions/corral-test/herdr.sock");
    assert_eq!(health["selected_socket"], json!(socket));
    store
        .bind_herdr_session(&socket, "Retained original launch record")
        .unwrap();
    assert_eq!(brief(temp.path())["health"]["sync_owned"], true);
}

#[test]
fn pm_brief_reports_done_workers_and_reviewer_children_without_finish_records() {
    // Given Done tasks with unfinished workers or reviewer accounting children.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let worker = add_run(&store, Status::Done, json!({}));
    let parent = add_run(
        &store,
        Status::Done,
        json!({
            "ended":"2026-10-15T01:00:00Z", "outcome":"done",
            "review_runs":[{
                "id":"R1-review-1", "agent":"generic", "worktree":temp.path().join("missing"),
                "branch":"corral/fixture", "base":"main", "base_sha":"abc",
                "started":"2026-10-15T00:00:00Z"
            }]
        }),
    );
    add_run(
        &store,
        Status::Done,
        json!({"ended":"2026-10-15T01:00:00Z","outcome":"done"}),
    );
    // When reading health from the actual PM CLI.
    let health = brief(temp.path())["health"].clone();
    // Then it names only unfinished runs, including the child, without inventing outcomes.
    assert_eq!(
        health["done_unfinished_runs"],
        json!([{"task":worker,"run":"R1"},{"task":parent,"run":"R1-review-1"}])
    );
    assert_eq!(health["sync_owned"], true);
    assert!(store.get(&worker).unwrap().runs[0].outcome.is_none());
}

#[test]
fn pm_brief_exposes_latest_retained_version_skew_with_event_identity() {
    // Given skew reports followed by an unrelated event.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    store
        .log_event(None, "version_skew", json!({"pid":41}))
        .unwrap();
    store
        .log_event(None, "version_skew", json!({"pid":42}))
        .unwrap();
    store.log_event(None, "unrelated", json!({})).unwrap();
    let events = std::fs::read(store.dir().join("events.jsonl")).unwrap();
    // When inspecting the new binary's PM brief.
    let health = brief(temp.path())["health"].clone();
    // Then the historical signal retains its timestamp and PID without replaying a sync.
    assert_eq!(health["last_version_skew"]["kind"], "version_skew");
    assert_eq!(health["last_version_skew"]["detail"]["pid"], 42);
    assert!(health["last_version_skew"]["ts"].is_string());
    assert_eq!(
        events,
        std::fs::read(store.dir().join("events.jsonl")).unwrap()
    );
}

#[test]
fn pm_brief_empty_store_has_no_health_incidents() {
    // Given a new store without any launched panes or recorded skew.
    let temp = tempfile::tempdir().unwrap();
    Store::init(temp.path()).unwrap();
    // When inspecting health.
    let health = brief(temp.path())["health"].clone();
    // Then an empty store needs no migration.
    assert_eq!(health["sync_owned"], true);
    assert_eq!(health["done_unfinished_runs"], json!([]));
    assert!(health["last_version_skew"].is_null());
}
