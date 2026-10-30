//! Blocked is a read-only projection: legacy storage remains compatible.
use std::path::Path;
use std::process::{Command, Stdio};

use corral_core::{Run, Status, Store};
use serde_json::{Value, json};

fn cli(root: &Path, args: &[&str]) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("CORRAL_") || name.starts_with("HERDR_") {
            command.env_remove(key);
        }
    }
    let output = command
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .args(["--root", root.to_str().unwrap()])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn fixture() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("Stopped worker", None, Status::Running, "")
        .unwrap();
    let run: Run = serde_json::from_value(json!({
        "id":"R1", "agent":"generic", "harness":"generic",
        "worktree":tmp.path(), "branch":"corral/T-001", "base":"main", "base_sha":"fixture",
        "started":"2026-10-29T01:00:00Z", "agent_state":"working", "seen_working":true
    }))
    .unwrap();
    store
        .update(&task.id, |task| {
            task.runs.push(run);
            Ok(())
        })
        .unwrap();
    (tmp, store)
}

fn wait_on_dependency(store: &Store, status: Status) {
    store.create("Dependency", None, status, "").unwrap();
    store
        .update("T-001", |task| {
            task.current_run_mut()
                .unwrap()
                .obligations
                .waits
                .push(serde_json::from_value(json!({
                    "request":{
                        "version":1,"id":"landed","run":"R1",
                        "condition":"Dependency landing","owner":"PM","source":"worker",
                        "handle":{"kind":"channel","channel_id":"task:T-002:landed","capability":"wait_v1"}
                    },
                    "registered_at":"2026-10-29T01:15:00Z",
                    "observation":{"at":"2026-10-29T01:15:00Z","state":"pending","detail":""},
                    "receipt":null
                }))?);
            Ok(())
        })
        .unwrap();
}

#[test]
fn blocked_filter_includes_parked_run_and_preserves_legacy_storage() {
    // Given an idle native landing wait, despite old working telemetry.
    let (tmp, store) = fixture();
    wait_on_dependency(&store, Status::Ready);
    let before = store.get("T-001").unwrap();
    // When the real list CLI filters the derived column.
    let rows = cli(tmp.path(), &["list", "--status", "blocked", "--json"]);
    // Then the wait, its responsible task and its original age anchor survive the projection.
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["id"], "T-001");
    assert_eq!(rows[0]["status"], "running");
    assert_eq!(rows[0]["display_status"], "Blocked");
    assert_eq!(rows[0]["blocked"]["kind"], "obligation_wait");
    assert_eq!(rows[0]["blocked"]["since"], "2026-10-29T01:15:00Z");
    assert!(rows[0]["blocked"]["age_secs"].as_i64().unwrap() > 0);
    let dependency = rows[0]["blocked"]["resume"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["kind"] == "task_status")
        .unwrap();
    assert_eq!(dependency["target"], "T-002");
    assert_eq!(dependency["satisfied"], false);
    assert!(
        cli(tmp.path(), &["list", "--status", "running", "--json"])
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.get("T-001").unwrap(), before);
}

#[test]
fn landed_dependency_without_receipt_is_not_reported_as_live_work() {
    // Given a dependency that landed after the wait's last pending observation.
    let (tmp, store) = fixture();
    wait_on_dependency(&store, Status::Done);
    // When list and PM next read the current canonical state without syncing it.
    let rows = cli(tmp.path(), &["list", "--status", "blocked", "--json"]);
    let next = cli(tmp.path(), &["pm", "next", "--json"]);
    // Then the missing receipt is a PM blocker, not an owner question or an empty event wait.
    assert_eq!(rows[0]["id"], "T-001");
    assert_eq!(rows[0]["blocked"]["responsible"], "pm");
    let dependency = rows[0]["blocked"]["resume"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["kind"] == "task_status")
        .unwrap();
    assert_eq!(dependency["satisfied"], true);
    assert_eq!(next["task"], "T-001");
    assert_eq!(next["requires_owner"], false);
    assert!(next["command"].as_str().unwrap().contains("show"));
}

#[test]
fn pending_plan_without_reviewer_appears_in_blocked_filter() {
    // Given a requested review without a launched reviewer.
    let (tmp, store) = fixture();
    store
        .update("T-001", |task| {
            task.status = Status::Review;
            task.runs.clear();
            task.plans.push(serde_json::from_value(json!({
                "id":"P1","spec_revision":"fixture",
                "artifact":{
                    "run":"P1","kind":"plan","title":"Design","path":"plan.md",
                    "source":"/fixture/plan.md","sha256":"a","bytes":1,"added":"2026-10-29T01:00:00Z"
                },
                "document":{
                    "goal":"A tested change","non_goals":[],"decisions":[],
                    "affected_paths":["src/main.rs"],"contracts":[],"dependencies":[],
                    "acceptance":[{"scenario":"works","expected":"done"}],
                    "verification":["cargo test"],"owner_decisions":[]
                },
                "reviews":[{
                    "id":"PR1","reviewer":"fixture","policy_digest":"fixture",
                    "at":"2026-10-29T01:15:00Z"
                }]
            }))?);
            Ok(())
        })
        .unwrap();
    // When the requested review is projected.
    let rows = cli(tmp.path(), &["list", "--status", "blocked", "--json"]);
    // Then it cannot disappear into Review as if a reviewer were running.
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["id"], "T-001");
    assert_eq!(rows[0]["display_status"], "Blocked");
    assert!(!rows[0]["unblocker"].as_str().unwrap().is_empty());
}

#[test]
fn escalation_remains_blocked_while_working_worker_does_not() {
    // Given a genuine working worker and another run escalated to the owner.
    let (tmp, store) = fixture();
    let escalated = store
        .create("Owner decision", None, Status::NeedsMe, "")
        .unwrap();
    let mut run = store.get("T-001").unwrap().current_run().unwrap().clone();
    run.outcome = Some(corral_core::RunOutcome::Done);
    run.autopilot = Some(
        serde_json::from_value(json!({
            "stage":"escalated","at":"2026-10-29T01:30:00Z","reason":"Choose the delivery scope"
        }))
        .unwrap(),
    );
    store
        .update(&escalated.id, |task| {
            task.runs.push(run);
            Ok(())
        })
        .unwrap();
    // When the real CLI filters Blocked.
    let rows = cli(tmp.path(), &["list", "--status", "blocked", "--json"]);
    // Then only the stopped run is included, with owner responsibility.
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["id"], "T-002");
    assert_eq!(rows[0]["status"], "needs_me");
    assert_eq!(rows[0]["blocked"]["kind"], "escalation");
    assert_eq!(rows[0]["blocked"]["responsible"], "owner");
}
