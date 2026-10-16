use std::process::{Command, Output, Stdio};

use corral_core::{ArtifactKind, Status, Store};
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

    fn report(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).unwrap()
    }
}

#[test]
fn report_is_pinned_and_readable_by_existing_markdown_viewer() {
    // Given a recorded landing, reviewer, summary and cost.
    let fx = Fixture::new();
    let task = fx
        .store
        .create("Landed capability", None, Status::Done, "")
        .unwrap();
    fx.store.update(&task.id, |task| {
        task.runs.push(serde_json::from_value(json!({
            "id":"R1", "agent":"worker", "worktree":"/not-read", "branch":"unused",
            "base":"main", "base_sha":"base", "started":"2026-01-01T00:00:00Z",
            "ended":"2026-01-01T00:05:00Z", "summary":"You can now inspect sprint outcomes.\nVerified: fixture",
            "review":{"reviewer":"pm","sha":"abc","at":"2026-01-01T00:04:00Z","verdict":"approve"},
            "usage":{"input":10,"output":5,"cache_read":0,"cache_write":0,"reported_usd":0.25}
        }))?);
        Ok(())
    }).unwrap();
    fx.store
        .log_event(
            Some(&task.id),
            "approved",
            json!({"sha":"abc","run":"R1","strategy":"rebase"}),
        )
        .unwrap();
    // When producing JSON and then reading its Markdown artifact through T-060.
    let report = fx.report(&["review-sprint", "--json"]);
    let reference = report["artifact_ref"].as_str().unwrap();
    let raw = fx.ok(&["artifact", "show", reference, "--raw"]);
    let rendered = fx.ok(&["artifact", "show", reference]);
    // Then report bytes survive the viewer, and machine fields preserve evidence.
    assert_eq!(
        raw.trim_end(),
        report["markdown"].as_str().unwrap().trim_end()
    );
    assert!(!rendered.is_empty());
    assert_eq!(report["sections"]["landed"][0]["sha"], "abc");
    assert_eq!(report["sections"]["landed"][0]["verdict"], "approve");
    assert_eq!(
        report["sections"]["numbers"]["by_task"][&task.id]["reported_usd"],
        0.25
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
fn mark_excludes_old_outcomes_and_since_can_override_it() {
    // Given an owner reduction acknowledged in a marked report.
    let fx = Fixture::new();
    fx.ok(&["new", "Old idea", "--idea"]);
    fx.ok(&["move", "1", "dropped", "--owner-ok"]);
    let first = fx.report(&["review-sprint", "--mark", "--json"]);
    // When a newer owner outcome is followed by a default report.
    fx.ok(&["new", "New idea", "--idea"]);
    fx.ok(&[
        "idea",
        "2",
        "decline",
        "--owner-ok",
        "--reason",
        "Owner reconsidered",
    ]);
    let next = fx.report(&["review-sprint", "last", "--json"]);
    let all = fx.report(&["review-sprint", "--since", "2020-01-01T00:00:00Z", "--json"]);
    // Then history is strict, while all owner ideas remain in the current catalog.
    assert_eq!(next["since"], first["snapshot_at"]);
    let outcomes = next["sections"]["dropped_rejected_escalated"]
        .as_array()
        .unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0]["task"], "T-002");
    assert_eq!(next["sections"]["owner_ideas"].as_array().unwrap().len(), 2);
    assert_eq!(
        all["sections"]["dropped_rejected_escalated"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn owner_drop_is_one_approved_outcome_not_an_unapproved_duplicate() {
    // Given a protected owner idea and an ordinary PM idea.
    let fx = Fixture::new();
    fx.ok(&["new", "Owner idea", "--idea"]);
    fx.ok(&["new", "PM idea", "--idea", "--origin", "pm"]);
    fx.ok(&["idea", "2", "decline", "--reason", "Not needed"]);
    // When the owner approves a drop through the manual move path.
    fx.ok(&[
        "move",
        "1",
        "dropped",
        "--owner-ok",
        "--reason",
        "Superseded",
    ]);
    let report = fx.report(&["review-sprint", "--json"]);
    // Then the owner item leads with its actual permission and reason, without duplication.
    let outcomes = report["sections"]["dropped_rejected_escalated"]
        .as_array()
        .unwrap();
    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes[0]["task"], "T-001");
    assert_eq!(outcomes[0]["owner_ok"], true);
    assert_eq!(outcomes[0]["reason"], "Superseded");
    assert_eq!(outcomes[1]["task"], "T-002");
}

#[test]
fn invalid_window_does_not_create_a_report_or_mark() {
    // Given an empty workspace.
    let fx = Fixture::new();
    let before = fx.store.events().unwrap();
    // When supplying invalid or conflicting window selectors.
    assert!(
        !fx.run(&["review-sprint", "--since", "bad", "--mark"])
            .status
            .success()
    );
    assert!(
        !fx.run(&["review-sprint", "last", "--since", "2020-01-01T00:00:00Z"])
            .status
            .success()
    );
    // Then parsing and validation have no report side effects.
    assert!(fx.store.list().unwrap().is_empty());
    assert_eq!(fx.store.events().unwrap(), before);
}

#[test]
fn non_owner_manual_drop_keeps_reason_and_defaults_decision_author_to_owner() {
    // Given a PM-filed task and a human deciding to drop it.
    let fx = Fixture::new();
    fx.ok(&["new", "PM request", "--origin", "pm"]);
    // When the human supplies a reason without an owner-consent flag.
    fx.ok(&[
        "move",
        "1",
        "dropped",
        "--reason",
        "Superseded by shared implementation",
    ]);
    // Then the report retains the reason and decider, independently of filing origin.
    let report = fx.report(&["review-sprint", "--json"]);
    let outcome = &report["sections"]["dropped_rejected_escalated"][0];
    assert_eq!(outcome["reason"], "Superseded by shared implementation");
    assert_eq!(outcome["actor"], "owner");
    assert_eq!(
        fx.store.get("1").unwrap().effective_origin(),
        corral_core::Origin::Pm
    );
    let moved = fx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|event| event.kind == "moved")
        .unwrap();
    assert_eq!(moved.detail["reason"], outcome["reason"]);
    assert_eq!(moved.detail["actor"], outcome["actor"]);
}

#[test]
fn manual_drop_uses_explicit_decision_author_instead_of_filing_origin() {
    // Given an agent-filed task and an explicit PM decision.
    let fx = Fixture::new();
    fx.ok(&["new", "Agent request", "--origin", "agent"]);
    // When the PM drops it with its actual decision author and reason.
    fx.ok(&[
        "move",
        "1",
        "dropped",
        "--author",
        "pm",
        "--reason",
        "Replaced by planned work",
    ]);
    // Then the report names the PM, not the filing agent or the CLI's human default.
    let report = fx.report(&["review-sprint", "--json"]);
    let outcome = &report["sections"]["dropped_rejected_escalated"][0];
    assert_eq!(outcome["reason"], "Replaced by planned work");
    assert_eq!(outcome["actor"], "pm");
    assert_eq!(
        fx.store.get("1").unwrap().effective_origin(),
        corral_core::Origin::Agent
    );
}
