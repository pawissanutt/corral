//! Board updates include usage refresh, so observations must survive both writers.
//! Commands submitted to herdr are recorded, never executed.
#![cfg(unix)] // Every fixture executes the symlinked POSIX fake engine.

use std::path::Path;

use corral_core::{Accept, AgentCmd, AutoStage, GateRecord, GateResult, Run, Status, Store, Task};
use corral_herdr::Herdr;
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use super::run;

#[test]
fn freshly_started_listed_gate_survives_board_sync_and_usage_refresh() {
    let fx = Fixture::new();
    fx.step();

    // A CLI service starts the gate between board updates; the board has never seen it.
    let (id, launched) = fx.start_gate();
    assert_eq!(fx.ctx.herdr.pane_list().unwrap()[0].pane_id, "w3A:p9");
    fx.step();
    fx.assert_gate_pending(&id, &launched);
    fx.step();
    fx.assert_gate_pending(&id, &launched);
}

#[test]
fn minutes_old_listed_gate_keeps_its_slot_across_board_updates() {
    let fx = Fixture::new();
    fx.step();
    let (id, launched) = fx.start_gate();
    fx.age(&id);
    let old = fx
        .persisted(&id)
        .current_run()
        .unwrap()
        .gate
        .as_ref()
        .unwrap()
        .at;

    // Exceed grace by editing persisted timestamps, never waiting on wall-clock time.
    fx.step();
    fx.assert_gate_pending(&id, &launched);
    fx.step();
    fx.assert_gate_pending(&id, &launched);
    assert_eq!(
        fx.persisted(&id)
            .current_run()
            .unwrap()
            .gate
            .as_ref()
            .unwrap()
            .at,
        old
    );
}

#[test]
fn transient_gate_listing_miss_does_not_release_or_escalate() {
    let fx = Fixture::new();
    fx.step();
    let (id, launched) = fx.start_gate();
    fx.age(&id);
    fx.step();
    fx.assert_gate_pending(&id, &launched);

    // pane get remains authoritative; only the listing briefly omits this qualified ID.
    fx.listing_miss("gate", true);
    fx.step();
    fx.assert_gate_pending(&id, &launched);
    fx.listing_miss("gate", false);
    fx.step();
    fx.assert_gate_pending(&id, &launched);

    // Presence must reset any persisted miss count: another isolated miss is harmless.
    fx.listing_miss("gate", true);
    fx.step();
    fx.assert_gate_pending(&id, &launched);
    fx.listing_miss("gate", false);
    fx.step();
    fx.assert_gate_pending(&id, &launched);
}

#[test]
fn dispatched_agent_transient_listing_miss_does_not_flap_board_state() {
    let mut fx = Fixture::new();
    fx.ctx.store.config.dispatch.paused = false;
    fx.ctx.store.save_config().unwrap();
    fx.step();
    let id = fx
        .ctx
        .store
        .create("Pending agent", Some("app"), Status::Ready, "")
        .unwrap()
        .id;
    let dispatched = corral_ops::dispatch(
        &fx.ctx,
        &id,
        &DispatchOpts {
            profile: Some("fake".into()),
            ..DispatchOpts::default()
        },
    )
    .unwrap();
    assert!(matches!(dispatched, DispatchOutcome::Started { .. }));
    let task = fx.persisted(&id);
    let started = task.current_run().unwrap();
    assert_eq!(started.herdr.as_ref().unwrap().pane_id, "w3D:p1");
    assert!(!started.worktree.join("agent-command-executed").exists());
    fx.age(&id);
    fx.step();

    for missing in [true, false, true, false] {
        fx.listing_miss("agent", missing);
        fx.step();
        let task = fx.persisted(&id);
        assert_eq!(task.status, Status::Running);
        let current = task.current_run().unwrap();
        assert!(current.outcome.is_none());
        assert!(current.seen_pane);
        assert!(current.blocked_excerpt.is_none());
        if !missing {
            assert_eq!(current.agent_state.as_deref(), Some("working"));
        }
        fx.assert_no_false_events(&id);
    }
}

struct Fixture {
    root: tempfile::TempDir,
    ctx: Ctx,
}

#[test]
fn unknown_inventory_defers_even_a_completed_gate_receipt_until_inventory_recovers() {
    // Given a gate whose result arrives between board updates.
    let mut fx = Fixture::new();
    fx.ctx.store.config.review.gate = Some(vec!["true".into()]);
    fx.ctx.store.save_config().unwrap();
    let (id, _) = fx.start_gate();
    fx.step();
    let task = fx.persisted(&id);
    let run = task.current_run().unwrap();
    let paths = corral_ops::gate_paths(&fx.ctx, &id, &run.id);
    let output = crate::board::tests::fixture_command("bash", fx.root.path())
        .arg(paths.script)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(paths.result.is_file());
    let before = fx.persisted(&id);

    // When the next inventory is UNKNOWN, not an authoritative observation.
    std::fs::write(fx.root.path().join("inventory-mode"), "empty\n").unwrap();
    fx.step();

    // Then that tick performs no task mutation or capacity release.
    assert_eq!(fx.persisted(&id), before);
    std::fs::remove_file(fx.root.path().join("inventory-mode")).unwrap();
    fx.step();
    assert_eq!(
        fx.persisted(&id)
            .current_run()
            .unwrap()
            .gate
            .as_ref()
            .unwrap()
            .result,
        GateResult::Pass
    );
}

#[test]
fn unknown_inventory_preserves_gate_state_and_warns_through_board_sink() {
    let fx = Fixture::new();
    let (id, gate) = fx.start_gate();
    fx.age(&id);
    fx.step();
    let before = fx.persisted(&id);
    let (sender, messages) = std::sync::mpsc::channel();
    let _capture = corral_ops::capture_messages(sender);

    // When repeated board updates cannot obtain a usable inventory.
    for mode in ["empty", "error", "timeout", "malformed"] {
        std::fs::write(fx.root.path().join("inventory-mode"), format!("{mode}\n")).unwrap();
        for _ in 0..2 {
            fx.step();
            // Then even an old gate keeps its slot, with a diagnostic in the board sink.
            fx.assert_gate_pending(&id, &gate);
            assert_eq!(fx.persisted(&id), before);
            assert!(
                messages
                    .try_iter()
                    .any(|message| message.source == "pane inventory")
            );
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = crate::board::tests::fixture_command("git", dir)
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().into()
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        std::fs::create_dir(&app).unwrap();
        git(&app, &["init", "-q", "-b", "main"]);
        git(&app, &["config", "user.name", "corral sync test"]);
        git(&app, &["config", "user.email", "sync@example.invalid"]);
        std::fs::write(app.join("tracked"), "initial\n").unwrap();
        git(&app, &["add", "tracked"]);
        git(&app, &["commit", "-qm", "initial"]);
        let bin = root.path().join("herdr");
        // A symlink to an immutable executable avoids ETXTBSY from parallel tests.
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app/sync_step_herdr.sh"),
            &bin,
        )
        .unwrap();
        let mut store = Store::init(root.path()).unwrap();
        store.config.herdr.session = Some("corral-test".into());
        store.config.dispatch.paused = true;
        store.config.sync.startup_grace_secs = 30;
        store.config.review.autopilot = Some(true);
        store.config.review.gate = Some(vec!["touch gate-command-executed".into()]);
        store.config.review.accept = Some(Accept::Owner);
        store.config.agents.insert(
            "fake".into(),
            AgentCmd {
                command: "touch agent-command-executed".into(),
            },
        );
        store.save_config().unwrap();
        Self {
            ctx: Ctx {
                resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
                store,
                herdr: Herdr::with_bin(bin, Some("corral-test".into())),
            },
            root,
        }
    }

    fn step(&self) {
        let changes = run(&self.ctx).unwrap();
        assert!(
            changes
                .iter()
                .all(|change| change.to != Status::NeedsMe
                    && change.stage != Some(AutoStage::Escalated)),
            "{changes:#?}"
        );
    }

    fn persisted(&self, id: &str) -> Task {
        // Re-open rather than trust an in-memory snapshot from the previous step.
        Store::open(self.root.path()).unwrap().get(id).unwrap()
    }

    fn start_gate(&self) -> (String, GateRecord) {
        let app = self.root.path().join("app");
        let task = self
            .ctx
            .store
            .create("Pending gate", Some("app"), Status::Review, "")
            .unwrap();
        let now = Timestamp::now();
        let candidate: Run = serde_json::from_value(json!({
            "id": "R1", "agent": "fake", "worktree": app,
            "branch": "main", "base": "main",
            "base_sha": git(&app, &["rev-parse", "HEAD"]),
            "started": now, "ended": now, "outcome": "done",
            "herdr": {
                "workspace_id": "w3D", "tab_id": "w3D:t1", "pane_id": "w3D:p1",
                "socket": self.ctx.herdr.socket_path().unwrap()
            }
        }))
        .unwrap();
        self.ctx
            .store
            .update(&task.id, |task| {
                task.runs.push(candidate);
                Ok(())
            })
            .unwrap();
        let record = corral_ops::gate_start(&self.ctx, &task.id).unwrap();
        assert_eq!(record.pane.as_deref(), Some("w3A:p9"));
        assert!(record.identity.is_some());
        (task.id, record)
    }

    fn age(&self, id: &str) {
        let old = Timestamp::now() - SignedDuration::from_secs(600);
        self.ctx
            .store
            .update(id, |task| {
                let run = task.current_run_mut().unwrap();
                run.started = old;
                if let Some(gate) = &mut run.gate {
                    gate.at = old;
                }
                Ok(())
            })
            .unwrap();
    }

    fn listing_miss(&self, kind: &str, missing: bool) {
        let path = self.root.path().join(format!("miss-{kind}"));
        if missing {
            std::fs::write(path, "").unwrap();
        } else {
            std::fs::remove_file(path).unwrap();
        }
    }

    fn assert_gate_pending(&self, id: &str, launched: &GateRecord) {
        let task = self.persisted(id);
        assert_eq!(task.status, Status::Review);
        let current = task.current_run().unwrap();
        let gate = current.gate.as_ref().unwrap();
        assert_eq!(gate.result, GateResult::Running);
        assert!(!gate.process_lost, "{gate:#?}");
        assert!(gate.holds_slot());
        assert_eq!(gate.pane, launched.pane);
        assert_eq!(gate.identity, launched.identity);
        assert_eq!(
            current.autopilot.as_ref().map(|state| state.stage),
            Some(AutoStage::Gating)
        );
        let paths = corral_ops::gate_paths(&self.ctx, id, &current.id);
        assert!(paths.script.is_file());
        assert!(!paths.result.exists());
        assert!(!current.worktree.join("gate-command-executed").exists());
        self.assert_no_false_events(id);
        let events = self.ctx.store.events().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.task.as_deref() == Some(id) && event.kind == "gate_started")
                .count(),
            1
        );
    }

    fn assert_no_false_events(&self, id: &str) {
        for event in self.ctx.store.events().unwrap() {
            if event.task.as_deref() != Some(id) {
                continue;
            }
            assert!(
                !matches!(
                    event.kind.as_str(),
                    "review_released" | "needs_me" | "resumed" | "autopilot_error"
                ),
                "{event:#?}"
            );
            if event.kind == "autopilot" {
                assert_ne!(event.detail["to"], "escalated", "{event:#?}");
            }
        }
    }
}
