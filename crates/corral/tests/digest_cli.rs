use std::process::{Command, Output, Stdio};

use corral_core::{ArtifactKind, Status, Store};
use jiff::Timestamp;
use serde_json::{Value, json};

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        Self { dir, store }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("CORRAL_")
                || key.to_string_lossy().starts_with("HERDR_")
            {
                cmd.env_remove(key);
            }
        }
        cmd.env("CORRAL_ROOT", self.dir.path())
            .env("HERDR_BIN_PATH", self.dir.path().join("no-herdr"))
            .current_dir(self.dir.path())
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}

#[test]
fn digest_cli_emits_json_and_a_pinned_readable_artifact() {
    // Given a saved landing, reviewer and usage.
    let fx = Fixture::new();
    let task = fx.store.create("landed", None, Status::Done, "").unwrap();
    fx.store
        .update(&task.id, |task| {
            task.runs.push(serde_json::from_value(json!({
            "id":"R1","agent":"worker","worktree":"/not-read","branch":"unused",
            "base":"main","base_sha":"base","started":"2026-01-01T00:00:00Z",
            "ended":"2026-01-01T00:05:00Z",
            "review":{"reviewer":"pm","sha":"abc","at":"2026-01-01T00:04:00Z","verdict":"approve"},
            "usage":{"input":10,"output":5,"cache_read":0,"cache_write":0,"reported_usd":0.25}
        }))?);
            Ok(())
        })
        .unwrap();
    fx.store
        .log_event(
            Some(&task.id),
            "approved",
            json!({"sha":"abc","run":"R1","strategy":"rebase"}),
        )
        .unwrap();
    // When using the actual binary and reading the result through the artifact CLI.
    let report: Value =
        serde_json::from_str(&fx.ok(&["digest", "--since", "999w", "--json"])).unwrap();
    let reference = report["artifact_ref"].as_str().unwrap();
    let raw = fx.ok(&["artifact", "show", reference, "--raw"]);
    // Then JSON, protected report bytes and the durable pin agree.
    assert_eq!(report["landed"][0]["sha"], "abc");
    assert_eq!(report["landed"][0]["task"], task.id);
    assert_eq!(report["landed"][0]["verdict"], "approve");
    assert_eq!(report["totals"]["reported_usd"], 0.25);
    assert_eq!(report["totals"]["wall_seconds"], 300);
    assert_eq!(
        raw.trim_end(),
        report["markdown"].as_str().unwrap().trim_end()
    );
    assert_eq!(
        fx.store.get(&task.id).unwrap().artifacts[0].kind,
        ArtifactKind::Report
    );
    assert!(
        fx.store
            .pins()
            .unwrap()
            .iter()
            .any(|pin| pin.target == reference)
    );
}

#[test]
fn digest_defaults_to_one_day_and_rejects_invalid_windows() {
    // Given an empty isolated store.
    let fx = Fixture::new();
    // When using the default window and then an invalid one.
    let report: Value = serde_json::from_str(&fx.ok(&["digest", "--json"])).unwrap();
    let invalid = fx.run(&["digest", "--since", "0d"]);
    // Then the window is exactly one day, with normal CLI validation failure.
    let since: Timestamp = report["since"].as_str().unwrap().parse().unwrap();
    let at: Timestamp = report["snapshot_at"].as_str().unwrap().parse().unwrap();
    assert_eq!(at.duration_since(since).as_secs(), 86400);
    assert!(!invalid.status.success());
    assert_eq!(report["landed"].as_array().unwrap().len(), 0);
}

#[test]
fn sync_generates_one_scheduled_report_without_a_herdr_server() {
    // Given an enabled schedule in an isolated store with no terminal engine.
    let fx = Fixture::new();
    fx.ok(&["config", "set", "digest.every", "1d"]);
    // When the real sync command runs twice.
    fx.ok(&["sync"]);
    fx.ok(&["sync"]);
    // Then the first sync pins a report and the second does not duplicate it.
    let scheduled: Vec<_> = fx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "digest" && e.detail["scheduled"] == true)
        .collect();
    assert_eq!(scheduled.len(), 1);
    assert_eq!(fx.store.pins().unwrap().len(), 1);
    assert_eq!(fx.store.list().unwrap().len(), 1);
}
