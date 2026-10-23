//! A harness that writes its session while dispatch is still launching it owns that session:
//! dispatch -> fake herdr `pane run` (the harness header lands here) -> run saved -> later overload.
#![cfg(unix)] // Fixtures execute shell scripts via POSIX shebangs.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

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
  "pane run") bash -c "${4%; true > *}" >/dev/null 2>&1; ok '{}' ;;
  "agent prompt") printf '%s\n' "$4" >> "$dir/prompts.log"; ok '{}' ;;
  *) ok '{}' ;;
esac
"#;

const LAUNCH: &str = r#"#!/usr/bin/env python3
import json
import socket
from pathlib import Path

root = Path(__file__).parent
with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as launch:
    launch.bind(str(root / "harness.sock"))
    launch.settimeout(10)
    launch.sendto(b"launch", str(root / "launch.sock"))
    timestamp = launch.recv(128).decode("ascii")
target = Path((root / "target").read_text())
target.parent.mkdir(parents=True, exist_ok=True)
target.write_text(json.dumps({
    "type": "session", "version": 3, "id": "launch",
    "timestamp": timestamp, "cwd": "/x",
}) + "\n")
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
    executable(&bin_dir.join("launch.py"), LAUNCH);
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
        ": {{model_args}}; python3 '{}'",
        bin_dir.join("launch.py").display()
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
            metadata: None,
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
    // Subscribe before dispatch. The harness writes its header during IPC, using
    // the exact prepared-run boundary rather than sampling a truncated wall clock.
    let launch = UnixDatagram::bind(bin_dir.join("launch.sock")).unwrap();
    launch
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let (outcome, prepared_at) = std::thread::scope(|scope| {
        let dispatch = scope.spawn(|| corral_ops::dispatch(&ctx, &task.id, &opts));
        let mut message = [0; 16];
        let (length, sender) = launch.recv_from(&mut message).unwrap();
        assert_eq!(&message[..length], b"launch");
        let prepared = ctx.store.get(&task.id).unwrap();
        let prepared_at = prepared.current_run().unwrap().started;
        launch
            .send_to(
                prepared_at.to_string().as_bytes(),
                sender.as_pathname().unwrap(),
            )
            .unwrap();
        (dispatch.join().unwrap().unwrap(), prepared_at)
    });
    let DispatchOutcome::Started { task, .. } = outcome else {
        panic!("expected Started");
    };
    let run = task.current_run().unwrap();
    assert_eq!(run.worktree, worktree);

    // The scenario under test: the harness header predates the recorded start of its own run.
    let first = std::fs::read_to_string(&session).unwrap();
    let header: Value = serde_json::from_str(first.lines().next().unwrap()).unwrap();
    let created: Timestamp = header["timestamp"].as_str().unwrap().parse().unwrap();
    assert_eq!(created, prepared_at);
    assert!(created < run.started, "{created} !< {}", run.started);
    let boundary: Timestamp = std::fs::read_to_string(
        ctx.store
            .state_dir()
            .join("provider-stall")
            .join(format!("{}-{}.launch", task.id, run.id)),
    )
    .unwrap()
    .parse()
    .unwrap();
    assert_eq!(created, boundary);

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
