//! A harness that writes its session while dispatch is still launching it owns that session:
//! dispatch -> fake herdr `pane run` (the harness header lands here) -> run saved -> later overload.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

use corral_core::{Profile, Status, Store};
use corral_herdr::Herdr;
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

const FAKE_HERDR: &str = r#"#!/usr/bin/env bash
if [ "$1" = --session ]; then shift 2; fi
dir="$(dirname "$0")"
ok() { printf '{"id":"fake","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create") ok '{"workspace":{"workspace_id":"w1"},"root_pane":{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1"}}' ;;
  "workspace list") ok '{"workspaces":[{"workspace_id":"w1","label":"fake"}]}' ;;
  "pane list") ok "{\"panes\":[{\"pane_id\":\"w1:p1\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\",\"agent_status\":\"$(cat "$dir/status")\"}]}" ;;
  "pane run") bash -c "$4" >/dev/null 2>&1; ok '{}' ;;
  "agent prompt") printf '%s\n' "$4" >> "$dir/prompts.log"; ok '{}' ;;
  *) ok '{}' ;;
esac
"#;

const LAUNCH: &str = r#"#!/bin/sh
target="$(cat "$(dirname "$0")/target")"
mkdir -p "$(dirname "$target")"
printf '{"type":"session","version":3,"id":"launch","timestamp":"%s","cwd":"/x"}\n' "$(date -u +%Y-%m-%dT%H:%M:%S.%3NZ)" > "$target"
"#;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_session_created_during_launch_is_owned_by_the_dispatched_run() {
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir_all(&app).unwrap();
    git(&app, &["init", "-q", "-b", "main"]);
    git(&app, &["config", "user.name", "corral test"]);
    git(&app, &["config", "user.email", "corral@test.invalid"]);
    std::fs::write(app.join("README.md"), "hello\n").unwrap();
    git(&app, &["add", "-A"]);
    git(&app, &["commit", "-q", "-m", "init"]);

    let bin_dir = tmp.path().join("fake-herdr");
    std::fs::create_dir_all(&bin_dir).unwrap();
    executable(&bin_dir.join("herdr"), FAKE_HERDR);
    executable(&bin_dir.join("launch.sh"), LAUNCH);
    std::fs::write(bin_dir.join("status"), "blocked").unwrap();
    let agent = tmp.path().join("omo-agent");
    let worktree = app.join(".worktrees/T-001");
    let slug = worktree
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', "-");
    let session = agent
        .join("sessions")
        .join(format!("--{slug}--"))
        .join("launch.jsonl");
    std::fs::write(bin_dir.join("target"), session.to_string_lossy().as_bytes()).unwrap();

    let mut store = Store::init(tmp.path()).unwrap();
    store.config.sync.omo_agent_dir = Some(agent);
    let command = format!(
        ": {{model_args}}; sh '{}'",
        bin_dir.join("launch.sh").display()
    );
    store.config.harnesses.insert(
        "omo".into(),
        serde_json::from_value(json!({ "command": command })).unwrap(),
    );
    store.config.profiles.insert(
        "agent".into(),
        Profile {
            harness: "omo".into(),
            model: None,
            effort: None,
            args: Vec::new(),
            vendor: None,
            escalate_to: None,
        },
    );
    store.save_config().unwrap();
    let ctx = Ctx {
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        store,
        herdr: Herdr::with_bin(bin_dir.join("herdr"), None),
    };
    let task = ctx
        .store
        .create("Launch me", Some("app"), Status::Ready, "## Goal\nRun.\n")
        .unwrap();
    let opts = DispatchOpts {
        profile: Some("agent".into()),
        ..DispatchOpts::default()
    };
    let DispatchOutcome::Started { task, .. } =
        corral_ops::dispatch(&ctx, &task.id, &opts).unwrap()
    else {
        panic!("expected Started");
    };
    let run = task.current_run().unwrap();
    assert_eq!(run.worktree, worktree);

    // The scenario under test: the harness header predates the recorded start of its own run.
    let first = std::fs::read_to_string(&session).unwrap();
    let header: Value = serde_json::from_str(first.lines().next().unwrap()).unwrap();
    let created: Timestamp = header["timestamp"].as_str().unwrap().parse().unwrap();
    assert!(created < run.started, "{created} !< {}", run.started);

    let fixture = format!(
        "{}/tests/fixtures/provider-stall/overloaded.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let error_at = run.started + SignedDuration::from_secs(10);
    let error = std::fs::read_to_string(fixture).unwrap();
    let error = error
        .lines()
        .last()
        .unwrap()
        .replace("2026-10-15T09:10:00.000Z", &error_at.to_string());
    std::fs::write(&session, format!("{first}{error}\n")).unwrap();
    let file = std::fs::File::options().write(true).open(&session).unwrap();
    file.set_modified(SystemTime::from(error_at)).unwrap();

    corral_ops::sync_at(&ctx, error_at + SignedDuration::from_secs(60)).unwrap();

    let events = ctx.store.events().unwrap();
    let retries: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "provider_retry")
        .collect();
    assert_eq!(retries.len(), 1);
    assert_eq!(retries[0].detail["attempt"], 1);
    assert_eq!(retries[0].detail["submitted"], true);
    let prompts = std::fs::read_to_string(bin_dir.join("prompts.log")).unwrap();
    assert_eq!(prompts.lines().count(), 1);
}
