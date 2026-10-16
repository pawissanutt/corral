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
fn rejected_review_move_cannot_authorize_a_later_pm_deferral() {
    // Given owner work that is ready, not in review.
    let fx = Fixture::new();
    fx.ok(&["new", "Owner ready work", "--ready"]);
    // When a rejected request for Review is followed by an unapproved PM deferral.
    let rejected = fx.run(&["move", "1", "review", "--owner-ok"]);
    let deferred = fx.run(&[
        "move",
        "1",
        "backlog",
        "--author",
        "pm",
        "--reason",
        "PM defers without owner permission",
    ]);
    // Then both moves fail and no reusable owner authorization was recorded.
    assert!(!rejected.status.success());
    assert!(!deferred.status.success());
    assert_eq!(fx.store.get("1").unwrap().status, Status::Ready);
    assert!(
        fx.store
            .events()
            .unwrap()
            .iter()
            .all(|e| e.kind != "owner_decision")
    );
}

#[test]
fn unrelated_and_noop_moves_do_not_record_owner_authorization() {
    for (initial, target, force) in [
        (Status::Ready, "ready", false),
        (Status::Backlog, "ready", false),
        (Status::Ready, "review", true),
        (Status::Dropped, "dropped", false),
    ] {
        // Given an owner task with no pending approval.
        let fx = Fixture::new();
        fx.store
            .create_with_origin("Owner work", None, initial, "", Origin::Owner)
            .unwrap();
        let mut args = vec!["move", "1", target, "--owner-ok"];
        if force {
            args.push("--force");
        }
        // When a no-op or non-protected transition is requested with the flag.
        fx.ok(&args);
        // Then the flag does not grant permission for a different reduction.
        assert!(
            fx.store
                .events()
                .unwrap()
                .iter()
                .all(|e| e.kind != "owner_decision")
        );
    }
}

#[test]
fn valid_deferrals_consume_only_the_requested_flag_or_comment_approval() {
    for source in ["flag", "comment"] {
        // Given owner work ready for execution and exact approval for its deferral.
        let fx = Fixture::new();
        fx.ok(&["new", "Owner ready work", "--ready"]);
        let mut args = vec!["move", "1", "backlog", "--reason", "Revisit next sprint"];
        if source == "flag" {
            args.push("--owner-ok");
        } else {
            fx.ok(&["comment", "1", "owner-ok: defer Revisit next sprint"]);
        }
        // When the specifically approved deferral is performed.
        fx.ok(&args);
        // Then its approval is consumed by that transition.
        assert_eq!(fx.store.get("1").unwrap().status, Status::Backlog);
        let events = fx.store.events().unwrap();
        let approval = events.iter().find(|e| e.kind == "owner_decision").unwrap();
        let audit = events.iter().find(|e| e.kind == "idea_decided").unwrap();
        assert_eq!(approval.detail["action"], "defer");
        assert_eq!(approval.detail["source"], source);
        assert_eq!(audit.detail["decision_at"], approval.ts.to_string());
        assert_eq!(audit.detail["action"], "defer");
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

#[test]
fn repeated_decline_after_reopening_accepts_fresh_identical_approval() {
    // Given a declined idea reopened with its historical disposition preserved.
    let fx = Fixture::new();
    fx.ok(&["new", "Reopened decline", "--idea"]);
    let decision = [
        "idea",
        "1",
        "decline",
        "--reason",
        "Not needed",
        "--owner-ok",
    ];
    fx.ok(&decision);
    fx.ok(&["move", "1", "idea", "--force"]);
    // When the owner freshly approves the same action and reason.
    fx.ok(&decision);
    // Then the new decline is authorized and both decision receipts remain.
    assert_eq!(fx.store.get("1").unwrap().status, Status::Dropped);
    let audits: Vec<_> = fx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "idea_decided")
        .collect();
    assert_eq!(audits.len(), 2);
    assert!(
        audits
            .iter()
            .all(|event| event.detail["action"] == "decline" && event.detail["owner_ok"] == true)
    );
    assert_ne!(
        audits[0].detail["decision_at"],
        audits[1].detail["decision_at"]
    );
}

#[test]
fn repeated_merge_after_reopening_accepts_fresh_identical_approval() {
    // Given a merged idea reopened without losing its original merge backlink.
    let fx = Fixture::new();
    fx.ok(&["new", "Reopened merge", "--idea"]);
    fx.ok(&["new", "Shared implementation"]);
    let decision = [
        "idea",
        "1",
        "merge",
        "--into",
        "2",
        "--reason",
        "Shared outcome",
        "--owner-ok",
    ];
    fx.ok(&decision);
    let original_backlink = fx.store.get("2").unwrap().comments[0].clone();
    fx.ok(&["move", "1", "idea", "--force"]);
    // When the owner approves the same merge, destination, and reason again.
    fx.ok(&decision);
    // Then the fresh merge succeeds while retaining the prior relationship history.
    let source = fx.store.get("1").unwrap();
    assert_eq!(source.status, Status::Dropped);
    assert_eq!(source.idea_decision.unwrap().into.as_deref(), Some("T-002"));
    let target = fx.store.get("2").unwrap();
    assert_eq!(target.comments[0], original_backlink);
    assert_eq!(target.comments.len(), 2);
    let merges: Vec<_> = fx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "idea_merged")
        .collect();
    assert_eq!(merges.len(), 2);
    assert!(merges.iter().all(|event| event.detail["into"] == "T-002"));
}

#[test]
fn reopened_dispositions_cannot_reuse_consumed_or_wrong_action_consent() {
    for action in ["decline", "merge"] {
        // Given a consumed approval and a reopened idea with its prior disposition.
        let fx = Fixture::new();
        fx.ok(&["new", "Protected reopened idea", "--idea"]);
        fx.ok(&["new", "Destination"]);
        let mut decision = vec!["idea", "1", action, "--reason", "Same parameters"];
        if action == "merge" {
            decision.extend(["--into", "2"]);
        }
        let mut approved = decision.clone();
        approved.push("--owner-ok");
        fx.ok(&approved);
        fx.ok(&["move", "1", "idea", "--force"]);
        fx.ok(&["comment", "1", "owner-ok: drop Different action"]);
        let before = fx.store.get("1").unwrap();
        // When the historical decline/merge is requested without fresh matching consent.
        let refused = fx.run(&decision);
        // Then neither the consumed permission nor a drop permission authorizes it.
        assert!(!refused.status.success());
        let after = fx.store.get("1").unwrap();
        assert_eq!(after.status, Status::Idea);
        assert_eq!(after.idea_decision, before.idea_decision);
    }
}
