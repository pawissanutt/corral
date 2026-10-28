//! Unknown engine inventories must not become evidence that every pane died.
#![cfg(unix)] // Every scenario invokes the symlinked POSIX engine fixture.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;

use corral_core::{Run, Status, Store};
use corral_herdr::Herdr;
use corral_ops::Ctx;
use serde_json::{Value, json};

const STARTED: &str = "2000-01-01T00:00:00Z";

mod resource_support;

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
        resource_support::configure(&mut store.config);
        store.config.herdr.session = Some("corral-test".into());
        store.config.review.autopilot = Some(false);
        store.config.sync.startup_grace_secs = 30;
        store.config.limits.running = 4;
        store.save_config().unwrap();
        let ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
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

    fn observed_workers(&self) -> PathBuf {
        let socket = self.ctx.herdr.socket_path().unwrap();
        self.ctx
            .store
            .edit_config(|doc| doc.unset("herdr.session").map(|_| ()))
            .unwrap();
        for task in self.ctx.store.list().unwrap() {
            self.ctx
                .store
                .update(&task.id, |task| {
                    task.status = Status::Running;
                    let run = task.current_run_mut().unwrap();
                    run.herdr.as_mut().unwrap().socket = Some(socket.clone());
                    run.seen_pane = true;
                    run.seen_working = true;
                    run.agent_state = Some("working".into());
                    run.gate = None;
                    run.review = None;
                    run.review_runs.clear();
                    Ok(())
                })
                .unwrap();
        }
        socket
    }

    fn add_task(&self, status: Status, pane: &str, seen: bool, gate: bool, review: bool) {
        let store = &self.ctx.store;
        let task = store.create("Inventory fixture", None, status, "").unwrap();
        let mut run = json!({
            "id": "R1", "agent": "generic", "harness": "generic",
            "worktree": store.root().join("unused-worktree"),
            "branch": "main", "base": "main", "base_sha": "fixture",
            "started": STARTED,
            "herdr": {"workspace_id": "w3A", "pane_id": pane,
                      "socket": self.ctx.herdr.socket_path().unwrap()},
            "seen_pane": seen, "seen_working": seen,
            "agent_state": if seen { "working" } else { "starting" }
        });
        if gate {
            run["gate"] = json!({
                "result": "running", "at": STARTED,
                "sha": "fixture", "pane": "w3A:p12",
                "socket": self.ctx.herdr.socket_path().unwrap(),
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

    fn invoke(&self, verb: &str) -> Output {
        self.invoke_from(verb, None, Some("corral-test"))
    }

    fn invoke_from(&self, verb: &str, source: Option<&Path>, target: Option<&str>) -> Output {
        self.invoke_args(&[verb], source, target)
    }

    fn invoke_args(&self, args: &[&str], source: Option<&Path>, target: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
        // Match isolated CLI conventions, including inherited gate identities,
        // socket selectors and pane IDs from the agent running this test.
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
                command.env_remove(name);
            }
        }
        if let Some(socket) = source {
            command.env("HERDR_SOCKET_PATH", socket);
        }
        if let Some(session) = target {
            command
                .env("CORRAL_HERDR_SESSION", session)
                .env("HERDR_SESSION", session);
        }
        command
            .args(["--root", self.ctx.store.root().to_str().unwrap()])
            .args(args)
            .current_dir(self.ctx.store.root())
            .env("CORRAL_ROOT", self.ctx.store.root())
            .env("HERDR_BIN_PATH", &self.herdr)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn legacy(&self) {
        for task in self.ctx.store.list().unwrap() {
            self.ctx
                .store
                .update(&task.id, |task| {
                    for run in &mut task.runs {
                        run.herdr.as_mut().unwrap().socket = None;
                        if let Some(gate) = &mut run.gate {
                            gate.socket = None;
                        }
                        for child in &mut run.review_runs {
                            child.herdr.as_mut().unwrap().socket = None;
                        }
                    }
                    Ok(())
                })
                .unwrap();
        }
    }

    fn bind(&self, socket: &Path) {
        let record = self.ctx.store.root().join("original-launch.txt");
        std::fs::write(
            &record,
            format!("Fixture launch registration: {}\n", socket.display()),
        )
        .unwrap();
        let output = self.invoke_args(
            &[
                "bind-session",
                "--socket",
                socket.to_str().unwrap(),
                "--launch-record",
                record.to_str().unwrap(),
            ],
            None,
            Some("corral-test"),
        );
        assert!(output.status.success(), "{output:?}");
        let reopened = Store::open_unobserved(self.ctx.store.root()).unwrap();
        let binding = reopened.herdr_session_binding().unwrap().unwrap();
        assert_eq!(binding.socket, socket);
        assert_eq!(
            binding.launch_evidence,
            std::fs::read_to_string(record).unwrap()
        );
    }

    fn state_snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        std::fs::read_dir(self.ctx.store.state_dir())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .map(|path| {
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect()
    }

    fn cli(&self, verb: &str) {
        let output = self.invoke(verb);
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
        // Two running workers and the pending reviewer; the first worker's running gate shares
        // its task slot (a gate never needs a second slot, so workers cannot deadlock it).
        assert_eq!(corral_ops::slots(&self.ctx).unwrap(), (3, 4));
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
    let execution = |mut snapshot: Value| {
        for task in snapshot.as_array_mut().unwrap() {
            task.as_object_mut().unwrap().remove("attention");
        }
        snapshot
    };
    let before = execution(fixture.snapshot());
    fixture.assert_capacity_held();
    let other_events = || {
        serde_json::to_value(
            fixture
                .ctx
                .store
                .events()
                .unwrap()
                .into_iter()
                .filter(|event| {
                    !matches!(
                        event.kind.as_str(),
                        "attention_opened"
                            | "attention_overdue"
                            | "attention_due"
                            | "notification_claimed"
                            | "owner_notified"
                    )
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    };
    let events = other_events();
    let expected: BTreeMap<_, BTreeMap<_, _>> =
        corral_ops::attention::board(&fixture.ctx, jiff::Timestamp::now())
            .unwrap()
            .tasks
            .into_iter()
            .map(|row| {
                (
                    row.task,
                    row.assessment
                        .conditions
                        .into_iter()
                        .filter_map(|condition| condition.key.map(|key| (key, condition.kind)))
                        .collect(),
                )
            })
            .collect();
    assert!(expected.values().any(|conditions| !conditions.is_empty()));
    let mut openings = BTreeMap::new();
    let mut assert_attention = || {
        for task in fixture.ctx.store.list().unwrap() {
            let retained: BTreeMap<_, _> = task
                .attention
                .records
                .iter()
                .map(|(key, record)| (key.clone(), record.kind.clone()))
                .collect();
            assert_eq!(retained, expected[&task.id]);
            for (key, record) in &task.attention.records {
                let opened = (record.opened_at, record.first_recorded);
                assert_eq!(*openings.entry(key.clone()).or_insert(opened), opened);
                assert!(record.handling.is_none() && record.resolved_at.is_none());
            }
            assert!(task.attention.outbox.is_empty());
        }
    };

    // When fresh CLI processes repeatedly sync through both entry points.
    for _ in 0..3 {
        for verb in ["sync", "hook"] {
            fixture.cli(verb);
            // Only canonical attention bookkeeping may change; execution and timestamps stay.
            assert_eq!(execution(fixture.snapshot()), before, "{mode}: {verb}");
            assert_attention();
            fixture.assert_capacity_held();
            assert_eq!(
                other_events(),
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
    assert_eq!(execution(fixture.snapshot()), before);
    assert_attention();
    assert_eq!(other_events(), events);
    fixture.assert_capacity_held();
    let events = fixture.ctx.store.events().unwrap();
    let claims: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "notification_claimed")
        .collect();
    let batches: BTreeSet<_> = claims
        .iter()
        .map(|event| event.detail["batch_id"].as_str().unwrap())
        .collect();
    assert_eq!(batches.len(), claims.len());
    let keys: BTreeSet<_> = expected
        .values()
        .flat_map(|conditions| conditions.keys().map(|key| key.0.as_str()))
        .collect();
    assert!(claims.len() <= keys.len());
    for claim in claims {
        assert_eq!(claim.detail["producer"], "attention_ladder");
        let coverage = claim.detail["coverage"].as_array().unwrap();
        assert!(!coverage.is_empty());
        assert!(
            coverage
                .iter()
                .all(|entry| keys.contains(entry["condition_key"].as_str().unwrap()))
        );
    }
    let notified: BTreeSet<_> = events
        .iter()
        .filter(|event| event.kind == "owner_notified")
        .map(|event| event.detail["batch_id"].as_str().unwrap())
        .collect();
    assert_eq!(notified, batches);
    let calls = std::fs::read_to_string(fixture.ctx.store.root().join("calls")).unwrap();
    assert!(calls.lines().any(|call| call == "pane list"));
    assert_eq!(
        calls
            .lines()
            .filter(|call| call.starts_with("notification show "))
            .count(),
        batches.len()
    );
    assert!(
        calls
            .lines()
            .all(|call| matches!(call, "workspace list" | "pane list")
                || call.starts_with("notification show ")),
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

fn assert_foreign_session(legacy: bool) {
    // Given running workers, gate and reviewer owned by another herdr socket.
    let fixture = Fixture::new("foreign");
    if legacy {
        fixture.legacy();
    }
    let owner = Herdr::new(Some("default".into())).socket_path().unwrap();
    fixture
        .ctx
        .store
        .edit_config(|doc| doc.set_str("herdr.session", "default"))
        .unwrap();
    if !legacy {
        for task in fixture.ctx.store.list().unwrap() {
            fixture
                .ctx
                .store
                .update(&task.id, |task| {
                    for run in &mut task.runs {
                        run.herdr.as_mut().unwrap().socket = Some(owner.clone());
                        for child in &mut run.review_runs {
                            child.herdr.as_mut().unwrap().socket = Some(owner.clone());
                        }
                    }
                    Ok(())
                })
                .unwrap();
        }
        // Launch evidence, rather than a later config edit, remains authoritative.
        fixture
            .ctx
            .store
            .edit_config(|doc| doc.set_str("herdr.session", "corral-test"))
            .unwrap();
    }
    let before = fixture.snapshot();
    let events = serde_json::to_value(fixture.ctx.store.events().unwrap()).unwrap();
    let state = fixture.state_snapshot();
    // A foreign invocation must not even observe an unrelated pending config-file edit.
    let mut config = corral_core::Config::load(&fixture.ctx.store.config_path()).unwrap();
    config.sync.idle_after_secs += 1;
    std::fs::write(fixture.ctx.store.config_path(), config.to_toml()).unwrap();
    fixture.assert_capacity_held();

    // When corral-test's hook and manual sync repeatedly see a valid foreign inventory.
    for _ in 0..3 {
        for verb in ["hook", "sync"] {
            let output = fixture.invoke(verb);
            // Then the foreign invocation is a silent successful no-op, not a liveness sample.
            assert!(output.status.success(), "{verb}: {output:?}");
            assert!(output.stdout.is_empty(), "{verb}: {output:?}");
            assert!(output.stderr.is_empty(), "{verb}: {output:?}");
            assert_eq!(fixture.snapshot(), before);
            assert_eq!(fixture.state_snapshot(), state);
            fixture.assert_capacity_held();
            assert_eq!(
                serde_json::to_value(fixture.ctx.store.events().unwrap()).unwrap(),
                events
            );
        }
    }
    assert!(!fixture.ctx.store.root().join("calls").exists());
    assert!(
        !fixture
            .ctx
            .store
            .state_dir()
            .join("pane-liveness.json")
            .exists()
    );
}

#[test]
fn foreign_hook_and_sync_preserve_launch_session_states_events_and_slots() {
    assert_foreign_session(false);
}

#[test]
fn foreign_hook_and_sync_preserve_legacy_configured_session() {
    assert_foreign_session(true);
}

#[test]
fn editing_config_does_not_transfer_unbound_legacy_launch_ownership() {
    // Given old-format references launched before ownership was recorded.
    let fixture = Fixture::new("foreign");
    fixture.legacy();
    fixture
        .ctx
        .store
        .edit_config(|doc| doc.set_str("herdr.session", "default"))
        .unwrap();
    fixture
        .ctx
        .store
        .edit_config(|doc| doc.set_str("herdr.session", "corral-test"))
        .unwrap();
    let tasks = fixture.snapshot();
    let state = fixture.state_snapshot();
    let events = serde_json::to_value(fixture.ctx.store.events().unwrap()).unwrap();
    let slots = corral_ops::slots(&fixture.ctx).unwrap();

    // When both entrypoints select the newly configured foreign server.
    for verb in ["hook", "sync", "hook", "sync"] {
        let output = fixture.invoke(verb);
        // Then unknown launch ownership is a read-only successful no-op.
        assert!(output.status.success(), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert_eq!(fixture.snapshot(), tasks);
        assert_eq!(fixture.state_snapshot(), state);
        assert_eq!(
            serde_json::to_value(fixture.ctx.store.events().unwrap()).unwrap(),
            events
        );
        assert_eq!(corral_ops::slots(&fixture.ctx).unwrap(), slots);
    }
    assert!(!fixture.ctx.store.root().join("calls").exists());
}

#[test]
fn foreign_source_hook_cannot_be_retargeted_by_the_store_config() {
    // Given a store configured for corral-test but an event originating at another socket.
    let fixture = Fixture::new("foreign");
    let before = fixture.snapshot();
    let state = fixture.state_snapshot();
    let events = serde_json::to_value(fixture.ctx.store.events().unwrap()).unwrap();

    // When the hook's configured client could otherwise target the store's owning server.
    let output = fixture.invoke_from(
        "hook",
        Some(Path::new("/foreign/herdr.sock")),
        Some("corral-test"),
    );

    // Then even that retargeted event remains a silent no-op.
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(fixture.state_snapshot(), state);
    assert_eq!(
        serde_json::to_value(fixture.ctx.store.events().unwrap()).unwrap(),
        events
    );
    assert!(!fixture.ctx.store.root().join("calls").exists());
}

#[test]
fn same_session_hook_confirms_gone_only_after_two_misses() {
    // Given observed workers beyond startup grace, and another pane still alive on their server.
    let fixture = Fixture::new("foreign");
    let socket = fixture.observed_workers();

    // When two independent hook processes sample the owning server's missing panes.
    // Server events may carry a socket without HERDR_ENV or a client session override.
    let first = fixture.invoke_from("hook", Some(&socket), None);
    assert!(
        first.status.success() && first.stderr.is_empty(),
        "{first:?}"
    );
    assert!(
        fixture
            .ctx
            .store
            .list()
            .unwrap()
            .iter()
            .all(|t| t.status == Status::Running)
    );
    let second = fixture.invoke_from("hook", Some(&socket), None);

    // Then the second miss produces one gone transition per worker.
    assert!(second.status.success(), "{second:?}");
    assert!(second.stderr.is_empty(), "{second:?}");
    assert!(
        fixture.ctx.store.list().unwrap().iter().all(|t| {
            t.status == Status::NeedsMe
                && t.current_run().unwrap().agent_state.as_deref() == Some("gone")
        }),
        "tasks={} first={first:?} second={second:?} observations={:?}",
        fixture.snapshot(),
        std::fs::read_to_string(fixture.ctx.store.state_dir().join("pane-liveness.json"))
    );
    let events = fixture.ctx.store.events().unwrap();
    assert_eq!(events.iter().filter(|e| e.kind == "needs_me").count(), 3);
    for event in events.iter().filter(|e| e.kind == "needs_me") {
        assert_eq!(event.actor, Some(corral_core::Actor::System));
        assert_eq!(event.via.as_deref(), Some("hook"));
    }
    assert_eq!(events.iter().filter(|e| e.kind == "resumed").count(), 0);
}

#[test]
fn inventory_fixture_keeps_two_misses_independent_of_free_resource_changes() {
    // Given one healthy missing-pane sample in the same observed-worker fixture.
    let mut fixture = Fixture::new("foreign");
    fixture.observed_workers();
    let first = "2026-01-01T00:00:00Z".parse().unwrap();
    corral_ops::sync_at(&fixture.ctx, first).unwrap();
    assert!(
        fixture
            .ctx
            .store
            .list()
            .unwrap()
            .iter()
            .all(|t| t.status == Status::Running)
    );

    // When free memory and disk change before the second independent sample.
    let mut probe = corral_ops::resources::FixedResourceProbe::healthy();
    probe.0.mem_available = 1;
    for disk in &mut probe.0.disks {
        disk.free_bytes = 1;
    }
    fixture.ctx.resource_probe = std::sync::Arc::new(probe);
    corral_ops::sync_at(&fixture.ctx, "2026-01-01T00:00:01Z".parse().unwrap()).unwrap();

    // Then fixture capacity policy cannot conceal a confirmed missing worker.
    assert!(
        fixture.ctx.store.list().unwrap().iter().all(|t| {
            t.status == Status::NeedsMe
                && t.current_run().unwrap().agent_state.as_deref() == Some("gone")
        }),
        "{}",
        fixture.snapshot()
    );
}

#[test]
fn editing_config_does_not_transfer_bound_legacy_launch_ownership() {
    for original in ["default", "corral"] {
        // Given legacy panes bound from their original launch record, not config.
        let fixture = Fixture::new("foreign");
        fixture.legacy();
        let socket = Herdr::new(Some(original.into())).socket_path().unwrap();
        fixture.bind(&socket);
        let binding = fixture.ctx.store.herdr_session_binding().unwrap();
        fixture
            .ctx
            .store
            .edit_config(|doc| doc.set_str("herdr.session", "corral-test"))
            .unwrap();
        let tasks = fixture.snapshot();
        let state = fixture.state_snapshot();
        let events = serde_json::to_value(fixture.ctx.store.events().unwrap()).unwrap();
        let slots = corral_ops::slots(&fixture.ctx).unwrap();

        // When hook and manual sync use a newly configured foreign session.
        for verb in ["hook", "sync", "hook", "sync"] {
            let output = fixture.invoke(verb);
            // Then tasks, observations, events, ownership and capacity are unchanged.
            assert!(output.status.success(), "{original} {verb}: {output:?}");
            assert!(
                output.stdout.is_empty() && output.stderr.is_empty(),
                "{output:?}"
            );
            assert_eq!(fixture.snapshot(), tasks);
            assert_eq!(fixture.state_snapshot(), state);
            assert_eq!(
                serde_json::to_value(fixture.ctx.store.events().unwrap()).unwrap(),
                events
            );
            assert_eq!(corral_ops::slots(&fixture.ctx).unwrap(), slots);
            assert_eq!(fixture.ctx.store.herdr_session_binding().unwrap(), binding);
        }
        assert!(!fixture.ctx.store.root().join("calls").exists());
    }
}

#[test]
fn bound_legacy_corral_and_default_sessions_still_confirm_gone() {
    for original in ["corral", "default"] {
        // Given unconfigured old-format workers, observed long before startup grace.
        let fixture = Fixture::new("foreign");
        fixture.legacy();
        fixture
            .ctx
            .store
            .edit_config(|doc| doc.unset("herdr.session").map(|_| ()))
            .unwrap();
        for task in fixture.ctx.store.list().unwrap() {
            fixture
                .ctx
                .store
                .update(&task.id, |task| {
                    task.status = Status::Running;
                    let run = task.current_run_mut().unwrap();
                    run.seen_pane = true;
                    run.seen_working = true;
                    run.agent_state = Some("working".into());
                    run.gate = None;
                    run.review = None;
                    run.review_runs.clear();
                    Ok(())
                })
                .unwrap();
        }
        let socket = Herdr::new(Some(original.into())).socket_path().unwrap();
        fixture.bind(&socket);

        // When independent owning hooks miss the panes twice, with a live unrelated pane.
        let first = fixture.invoke_from("hook", Some(&socket), None);
        assert!(
            first.status.success() && first.stderr.is_empty(),
            "{first:?}"
        );
        let first_liveness =
            std::fs::read_to_string(fixture.ctx.store.state_dir().join("pane-liveness.json"));
        let first_resources =
            std::fs::read_to_string(fixture.ctx.store.state_dir().join("resources.json"));
        assert!(
            fixture
                .ctx
                .store
                .list()
                .unwrap()
                .iter()
                .all(|t| t.status == Status::Running)
        );
        let second = fixture.invoke_from("hook", Some(&socket), None);

        // Then the original corral and default servers both retain disappearance detection.
        assert!(
            second.status.success() && second.stderr.is_empty(),
            "{second:?}"
        );
        assert!(
            fixture
                .ctx
                .store
                .list()
                .unwrap()
                .iter()
                .all(|t| t.status == Status::NeedsMe),
            "{original}: tasks={} first_liveness={first_liveness:?} first_resources={first_resources:?} second_liveness={:?} second_resources={:?} calls={:?} first={first:?} second={second:?}",
            fixture.snapshot(),
            std::fs::read_to_string(fixture.ctx.store.state_dir().join("pane-liveness.json")),
            std::fs::read_to_string(fixture.ctx.store.state_dir().join("resources.json")),
            std::fs::read_to_string(fixture.ctx.store.root().join("calls"))
        );
        let events = fixture.ctx.store.events().unwrap();
        assert_eq!(events.iter().filter(|e| e.kind == "needs_me").count(), 3);
        assert_eq!(events.iter().filter(|e| e.kind == "resumed").count(), 0);
    }
}

#[test]
fn bound_legacy_manual_sync_uses_outside_herdr_corral_default() {
    // Given an unconfigured legacy store launched outside herdr on corral.
    let fixture = Fixture::new("foreign");
    fixture.legacy();
    fixture
        .ctx
        .store
        .edit_config(|doc| doc.unset("herdr.session").map(|_| ()))
        .unwrap();
    for task in fixture.ctx.store.list().unwrap() {
        fixture
            .ctx
            .store
            .update(&task.id, |task| {
                task.status = Status::Running;
                let run = task.current_run_mut().unwrap();
                run.seen_pane = true;
                run.seen_working = true;
                run.agent_state = Some("working".into());
                run.gate = None;
                run.review = None;
                run.review_runs.clear();
                Ok(())
            })
            .unwrap();
    }
    let socket = Herdr::new(Some("corral".into())).socket_path().unwrap();
    fixture.bind(&socket);

    // When manual sync has no environment selectors and samples two misses.
    let first = fixture.invoke_from("sync", None, None);
    assert!(
        first.status.success() && first.stderr.is_empty(),
        "{first:?}"
    );
    assert!(
        fixture
            .ctx
            .store
            .list()
            .unwrap()
            .iter()
            .all(|t| t.status == Status::Running)
    );
    let second = fixture.invoke_from("sync", None, None);

    // Then the supported outside-herdr default still reconciles its own legacy panes.
    assert!(
        second.status.success() && second.stderr.is_empty(),
        "{second:?}"
    );
    assert!(
        fixture
            .ctx
            .store
            .list()
            .unwrap()
            .iter()
            .all(|t| t.status == Status::NeedsMe)
    );
    assert_eq!(
        fixture
            .ctx
            .store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "needs_me")
            .count(),
        3
    );
}
