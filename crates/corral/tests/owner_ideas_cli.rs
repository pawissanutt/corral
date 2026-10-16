use std::process::{Command, Output, Stdio};

use corral_core::{Origin, Status, Store};
use serde_json::Value;

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
fn human_task_defaults_to_owner_and_explicit_origin_is_retained() {
    // Given a workspace and human/default, PM, and agent submissions.
    let fx = Fixture::new();
    fx.ok(&["new", "Human work"]);
    fx.ok(&["new", "PM idea", "--idea", "--origin", "pm"]);
    fx.ok(&["new", "Agent idea", "--idea", "--origin", "agent"]);
    // When they are read back through JSON.
    let tasks: Value = serde_json::from_str(&fx.ok(&["list", "--json"])).unwrap();
    // Then attribution survives creation even for non-owner ideas.
    assert_eq!(tasks[0]["origin"], "owner");
    assert_eq!(tasks[1]["origin"], "pm");
    assert_eq!(tasks[2]["origin"], "agent");
}

#[test]
fn forced_move_and_discard_refuse_unapproved_owner_drop() {
    // Given a human-filed idea.
    let fx = Fixture::new();
    fx.ok(&["new", "Keep my idea", "--idea"]);
    // When force or discard is used without consent.
    let forced = fx.run(&["move", "1", "dropped", "--force"]);
    let discarded = fx.run(&["discard", "1"]);
    // Then neither bypasses owner protection.
    assert!(!forced.status.success());
    assert!(!discarded.status.success());
    assert_eq!(fx.store.get("1").unwrap().status, Status::Idea);
}

#[test]
fn owner_ok_flag_permits_drop_and_records_decision() {
    // Given a human-filed idea.
    let fx = Fixture::new();
    fx.ok(&["new", "Superseded idea", "--idea"]);
    // When the owner explicitly approves the drop.
    fx.ok(&[
        "move",
        "1",
        "dropped",
        "--owner-ok",
        "--reason",
        "Use the replacement",
    ]);
    // Then both the status and explicit permission are durable.
    assert_eq!(fx.store.get("1").unwrap().status, Status::Dropped);
    let decision = fx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "owner_decision")
        .unwrap();
    assert_eq!(decision.detail["source"], "flag");
    assert_eq!(decision.detail["action"], "drop");
    assert_eq!(decision.detail["reason"], "Use the replacement");
}

#[test]
fn ordinary_owner_comment_is_not_blanket_permission() {
    // Given a task with unrelated owner context.
    let fx = Fixture::new();
    fx.ok(&["new", "Owner idea", "--idea"]);
    fx.ok(&["comment", "1", "Please keep the full scope"]);
    // When an agent attempts to decline it.
    let out = fx.run(&["idea", "1", "decline", "--reason", "Too much work"]);
    // Then unrelated owner prose cannot authorize a decline.
    assert!(!out.status.success());
    assert_eq!(fx.store.get("1").unwrap().status, Status::Idea);
}

#[test]
fn explicit_owner_comment_permits_only_named_action() {
    // Given explicit owner consent to decline an idea.
    let fx = Fixture::new();
    fx.ok(&["new", "Owner idea", "--idea"]);
    fx.ok(&["comment", "1", "owner-ok: decline Replaced by task two"]);
    // When that action is performed without another flag.
    fx.ok(&["idea", "1", "decline", "--reason", "Replaced by task two"]);
    // Then it succeeds with the original owner reason and attribution.
    assert_eq!(fx.store.get("1").unwrap().status, Status::Dropped);
    let audit = fx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "idea_decided")
        .unwrap();
    assert_eq!(audit.detail["actor"], "owner");
    assert_eq!(audit.detail["owner_ok"], true);
}

#[test]
fn pm_cannot_grant_owner_permission_in_a_comment() {
    // Given a PM-authored consent-shaped comment.
    let fx = Fixture::new();
    fx.ok(&["new", "Owner idea", "--idea"]);
    fx.ok(&[
        "comment",
        "1",
        "owner-ok: drop Not needed",
        "--author",
        "pm",
    ]);
    // When a caller attempts to use it.
    let out = fx.run(&["move", "1", "dropped"]);
    // Then the task is still protected.
    assert!(!out.status.success());
    assert_eq!(fx.store.get("1").unwrap().effective_origin(), Origin::Owner);
}

#[test]
fn merge_preserves_source_link_and_target_backlink() {
    // Given two owner tasks.
    let fx = Fixture::new();
    fx.ok(&["new", "Source idea", "--idea"]);
    fx.ok(&["new", "Combined implementation"]);
    // When the owner approves merging the idea into the implementation.
    fx.ok(&[
        "idea",
        "1",
        "merge",
        "--into",
        "2",
        "--reason",
        "Same outcome",
        "--owner-ok",
    ]);
    // Then the original idea stays addressable and the target records its source.
    let source = fx.store.get("1").unwrap();
    let target = fx.store.get("2").unwrap();
    assert_eq!(source.idea_decision.unwrap().into.as_deref(), Some("T-002"));
    assert_eq!(source.status, Status::Dropped);
    assert!(target.comments[0].text.contains("T-001"));
    let events = fx.store.events().unwrap();
    let merged = events.iter().find(|e| e.kind == "idea_merged").unwrap();
    assert_eq!(merged.task.as_deref(), Some("T-001"));
    assert_eq!(merged.detail["into"], "T-002");
}

#[test]
fn narrowing_requires_consent_and_keeps_owner_identity() {
    // Given an owner request and replacement specification.
    let fx = Fixture::new();
    fx.ok(&["new", "Full feature", "--idea", "--body", "Both behaviors"]);
    let path = fx.dir.path().join("narrowed.md");
    std::fs::write(&path, "One behavior\n").unwrap();
    let path = path.to_str().unwrap();
    let refused = fx.run(&[
        "idea",
        "1",
        "narrow",
        "--reason",
        "Reduce scope",
        "--body-file",
        path,
    ]);
    assert!(!refused.status.success());
    // When the owner approves the narrowed scope.
    fx.ok(&[
        "idea",
        "1",
        "narrow",
        "--reason",
        "Reduce scope",
        "--body-file",
        path,
        "--owner-ok",
    ]);
    // Then the change is explicit and remains owner-filed.
    let task = fx.store.get("1").unwrap();
    assert_eq!(task.body, "One behavior\n");
    assert_eq!(task.effective_origin(), Origin::Owner);
    assert_eq!(task.idea_decision.unwrap().action, "narrow");
}
