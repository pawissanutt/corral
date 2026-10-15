//! Unknown engine inventories must not become evidence that every pane died.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

use corral_core::{Run, Status, Store};
use corral_herdr::Herdr;
use corral_ops::Ctx;
use serde_json::{Value, json};

const STARTED: &str = "2000-01-01T00:00:00Z";

struct Fixture {
    _temp: tempfile::TempDir,
    ctx: Ctx,
    herdr: PathBuf,
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let herdr = temp.path().join("herdr");
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pane_inventory_herdr.sh"),
            &herdr,
        )
        .unwrap();
        std::fs::write(temp.path().join("mode"), format!("{mode}\n")).unwrap();
        let mut store = Store::init(temp.path()).unwrap();
        store.config.herdr.session = Some("corral-test".into());
        store.config.review.autopilot = Some(false);
        store.config.sync.startup_grace_secs = 30;
        store.config.limits.running = 4;
        store.save_config().unwrap();
        let ctx = Ctx {
            store,
            herdr: Herdr::with_bin(herdr.clone(), Some("corral-test".into())),
        };
        let fixture = Self {
            _temp: temp,
            ctx,
            herdr,
        };
        // Both starts predate grace by years. The second agent was never seen,
        // so even a missing-pane debounce cannot conceal an unknown inventory.
        fixture.add_task(Status::Running, "w3A:p9", true, true, false);
        fixture.add_task(Status::Running, "w3A:p10", false, false, false);
        fixture.add_task(Status::Review, "w3A:p11", true, false, true);
        fixture
    }

    fn add_task(&self, status: Status, pane: &str, seen: bool, gate: bool, review: bool) {
        let store = &self.ctx.store;
        let task = store.create("Inventory fixture", None, status, "").unwrap();
        let mut run = json!({
            "id": "R1", "agent": "generic", "harness": "generic",
            "worktree": store.root().join("unused-worktree"),
            "branch": "main", "base": "main", "base_sha": "fixture",
            "started": STARTED,
            "herdr": {"workspace_id": "w3A", "pane_id": pane},
            "seen_pane": seen, "seen_working": seen,
            "agent_state": if seen { "working" } else { "starting" }
        });
        if gate {
            run["gate"] = json!({
                "result": "running", "at": STARTED,
                "sha": "fixture", "pane": "w3A:p12",
                "identity": {
                    "version": 2, "task": task.id, "run": "R1",
                    "attempt": "inventory-gate", "head": "fixture",
                    "tree": "fixture", "integration_base": "fixture",
                    "dispatch_base": "fixture", "spec_digest": "fixture",
                    "policy_digest": "fixture", "inputs_digest": "fixture"
                }
            });
        }
        if review {
            run["review"] = json!({
                "reviewer": "generic", "sha": "fixture",
                "at": STARTED, "pane": "w3A:p13"
            });
            let mut child = run.clone();
            child["id"] = json!("R1-review");
            child["herdr"]["pane_id"] = json!("w3A:p13");
            child.as_object_mut().unwrap().remove("review");
            run["review_runs"] = json!([child]);
        }
        let run: Run = serde_json::from_value(run).unwrap();
        store
            .update(&task.id, |task| {
                task.runs.push(run.clone());
                Ok(())
            })
            .unwrap();
    }

    fn snapshot(&self) -> Value {
        serde_json::to_value(self.ctx.store.list().unwrap()).unwrap()
    }

    fn cli(&self, verb: &str) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
        // Match isolated CLI conventions, including inherited gate identities,
        // socket selectors and pane IDs from the agent running this test.
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
                command.env_remove(name);
            }
        }
        let output = command
            .args(["--root", self.ctx.store.root().to_str().unwrap(), verb])
            .current_dir(self.ctx.store.root())
            .env("CORRAL_ROOT", self.ctx.store.root())
            .env("HERDR_BIN_PATH", &self.herdr)
            .env("CORRAL_HERDR_SESSION", "corral-test")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{verb}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !output.stderr.is_empty(),
            "{verb} must diagnose an unknown inventory"
        );
        assert!(
            output.stdout.is_empty(),
            "{verb} reported a transition: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    fn assert_capacity_held(&self) {
        assert_eq!(corral_ops::slots(&self.ctx).unwrap(), (4, 4));
        let tasks = self.ctx.store.list().unwrap();
        let runs: Vec<_> = tasks.iter().flat_map(|task| &task.runs).collect();
        assert_eq!(
            runs.iter()
                .filter_map(|run| run.gate.as_ref())
                .filter(|gate| gate.holds_slot())
                .count(),
            1
        );
        assert_eq!(
            runs.iter()
                .filter_map(|run| run.review.as_ref())
                .filter(|review| review.holds_slot())
                .count(),
            1
        );
    }
}

fn assert_unknown_inventory(mode: &str) {
    // Given two old running agents plus a running gate and pending reviewer.
    let fixture = Fixture::new(mode);
    let before = fixture.snapshot();
    fixture.assert_capacity_held();
    let events = serde_json::to_value(fixture.ctx.store.events().unwrap()).unwrap();

    // When fresh CLI processes repeatedly sync through both entry points.
    for _ in 0..3 {
        for verb in ["sync", "hook"] {
            fixture.cli(verb);
            // Then no task/run field changes and no process capacity is released.
            assert_eq!(fixture.snapshot(), before, "{mode}: {verb}");
            fixture.assert_capacity_held();
            assert_eq!(
                serde_json::to_value(fixture.ctx.store.events().unwrap()).unwrap(),
                events,
                "{mode}: {verb} emitted a state event"
            );
        }
    }

    // Check the shared diagnostic sink's stable source, not warning prose.
    let (tx, rx) = mpsc::channel();
    {
        let _guard = corral_core::capture_messages(tx);
        assert!(corral_ops::sync(&fixture.ctx).unwrap().is_empty());
    }
    assert!(
        rx.try_iter()
            .any(|message| message.source == "pane inventory" && message.task.is_none()),
        "{mode}: missing shared inventory diagnostic"
    );
    assert_eq!(fixture.snapshot(), before);
    fixture.assert_capacity_held();
    let calls = std::fs::read_to_string(fixture.ctx.store.root().join("calls")).unwrap();
    assert!(calls.lines().any(|call| call == "pane list"));
    assert!(
        calls
            .lines()
            .all(|call| matches!(call, "workspace list" | "pane list")),
        "unexpected engine side effect: {calls}"
    );
}

#[test]
fn tasks_and_review_capacity_survive_nonzero_inventory() {
    assert_unknown_inventory("nonzero");
}

#[test]
fn tasks_and_review_capacity_survive_timeout_inventory() {
    assert_unknown_inventory("timeout");
}

#[test]
fn tasks_and_review_capacity_survive_malformed_success_inventory() {
    assert_unknown_inventory("malformed");
}

#[test]
fn tasks_and_review_capacity_survive_invalid_json_inventory() {
    assert_unknown_inventory("invalid-json");
}

#[test]
fn tasks_and_review_capacity_survive_empty_inventory() {
    assert_unknown_inventory("empty");
}
