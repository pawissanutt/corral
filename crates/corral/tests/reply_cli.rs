use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

use corral_core::{Run, Status, Store};
use serde_json::json;

#[path = "resource_support/mod.rs"]
mod resource_support;

// Avoid inheriting another fixture's writable executable fd across a concurrent fork.
static EXECUTABLE_FIXTURE: Mutex<()> = Mutex::new(());

struct Fixture {
    root: tempfile::TempDir,
    store: Store,
    herdr: std::path::PathBuf,
    _exclusive: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let exclusive = EXECUTABLE_FIXTURE.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::init(root.path()).unwrap();
        resource_support::configure(&mut store.config);
        store.save_config().unwrap();
        let task = store.create("Delivery", None, Status::NeedsMe, "").unwrap();
        let run: Run = serde_json::from_value(json!({
            "id": "R1", "agent": "omo", "harness": "omo", "worktree": root.path(),
            "branch": "corral/T-001", "base": "main", "base_sha": "abc",
            "started": "2026-10-15T09:00:00Z",
            "herdr": {"workspace_id": "w1", "pane_id": "w1:p1"},
            "blocked_excerpt": "question"
        }))
        .unwrap();
        store
            .update(&task.id, |t| {
                t.runs.push(run.clone());
                Ok(())
            })
            .unwrap();
        let herdr = root.path().join("herdr");
        std::fs::write(
            &herdr,
            r#"#!/bin/sh
dir="$(dirname "$0")"
if [ "$1" = --session ]; then shift 2; fi
case "$1 $2" in
  "agent prompt")
    if [ -f "$dir/fail" ]; then
      printf '%s\n' '{"error":{"code":"failed","message":"cannot deliver"}}'
      exit 1
    fi
    printf '%s\n' "$4" >> "$dir/delivered"
    printf '%s\n' '{"result":{}}' ;;
  "pane get") printf '%s\n' '{"result":{"pane":{"pane_id":"w1:p1"}}}' ;;
  *) exit 1 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            root,
            store,
            herdr,
            _exclusive: exclusive,
        }
    }

    fn reply(&self, args: &[&str]) -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
                command.env_remove(name);
            }
        }
        command
            .env("CORRAL_ROOT", self.root.path())
            .args([
                "--root",
                self.root.path().to_str().unwrap(),
                "reply",
                "T-001",
            ])
            .args(args)
            .env("HERDR_BIN_PATH", &self.herdr)
            .env("CORRAL_HERDR_SESSION", "fixture")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

#[test]
fn immediate_reply_records_delivery_and_clears_pending() {
    // Given a named agent that accepts prompts.
    let fx = Fixture::new();
    // When the CLI submits a reply.
    let out = fx.reply(&["continue"]);
    // Then delivery is recorded and no pending message survives.
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let task = fx.store.get("T-001").unwrap();
    assert_eq!(task.status, Status::Running);
    assert!(task.current_run().unwrap().pending_reply.is_none());
    let event = fx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "replied")
        .unwrap();
    assert_eq!(event.detail["via"], "agent_prompt");
    assert_eq!(event.detail["submitted"], true);
    assert_eq!(event.detail["attempts"], 1);
}

#[test]
fn failed_reply_is_visible_and_resend_delivers_stored_text() {
    // Given a failed agent prompt and an on-disk pending reply.
    let fx = Fixture::new();
    std::fs::write(fx.root.path().join("fail"), "").unwrap();
    let failed = fx.reply(&["continue"]);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("reply not delivered"));
    assert_eq!(
        fx.store
            .get("T-001")
            .unwrap()
            .current_run()
            .unwrap()
            .pending_reply
            .as_deref(),
        Some("continue")
    );
    let event = fx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "replied")
        .unwrap();
    assert_eq!(event.detail["via"], "agent_prompt");
    assert_eq!(event.detail["submitted"], false);
    assert_eq!(event.detail["attempts"], 1);
    std::fs::remove_file(fx.root.path().join("fail")).unwrap();
    // When the owner resends without supplying text.
    let out = fx.reply(&["--resend"]);
    // Then exactly one message is delivered and pending state is cleared.
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(fx.root.path().join("delivered")).unwrap(),
        "continue\n"
    );
    assert!(
        fx.store
            .get("T-001")
            .unwrap()
            .current_run()
            .unwrap()
            .pending_reply
            .is_none()
    );
}

#[test]
fn another_reply_cannot_silently_replace_undelivered_text() {
    // Given an undelivered reply.
    let fx = Fixture::new();
    std::fs::write(fx.root.path().join("fail"), "").unwrap();
    assert!(!fx.reply(&["first"]).status.success());
    // When a different reply is offered before resend.
    let out = fx.reply(&["second"]);
    // Then the first reply remains recoverable.
    assert!(!out.status.success());
    assert_eq!(
        fx.store
            .get("T-001")
            .unwrap()
            .current_run()
            .unwrap()
            .pending_reply
            .as_deref(),
        Some("first")
    );
}

#[test]
fn resend_requires_pending_text_and_conflicts_with_new_text() {
    // Given no pending reply.
    let fx = Fixture::new();
    // When resend is requested alone or with replacement text.
    let empty = fx.reply(&["--resend"]);
    let conflict = fx.reply(&["--resend", "new"]);
    // Then neither can type into the pane.
    assert!(!empty.status.success());
    assert!(!conflict.status.success());
    assert!(!fx.root.path().join("delivered").exists());
}

#[test]
fn failed_changes_are_retained_and_resend_records_delivery() {
    // Given a finished run awaiting changes and a temporarily failed transport.
    let fx = Fixture::new();
    fx.store
        .update("T-001", |t| {
            t.status = Status::Review;
            t.current_run_mut().unwrap().outcome = Some(corral_core::RunOutcome::Done);
            Ok(())
        })
        .unwrap();
    let ctx = corral_ops::Ctx {
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        store: fx.store.clone(),
        herdr: corral_herdr::Herdr::with_bin(fx.herdr.clone(), None),
    };
    std::fs::write(fx.root.path().join("fail"), "").unwrap();
    assert!(corral_ops::request_changes(&ctx, "T-001", "fix this").is_err());
    let pending = fx.store.get("T-001").unwrap();
    assert_eq!(pending.status, Status::Review);
    assert_eq!(
        pending.current_run().unwrap().pending_reply.as_deref(),
        Some("fix this")
    );
    let events = fx.store.events().unwrap();
    let failed = events
        .iter()
        .find(|e| e.kind == "changes_requested")
        .unwrap();
    assert_eq!(failed.detail["submitted"], false);
    std::fs::remove_file(fx.root.path().join("fail")).unwrap();
    // When the real resend CLI retries the pending change request.
    let out = fx.reply(&["--resend"]);
    // Then it reopens the same run and records actual delivery metadata.
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let task = fx.store.get("T-001").unwrap();
    assert_eq!(task.status, Status::Running);
    assert!(task.current_run().unwrap().pending_reply.is_none());
    assert!(task.current_run().unwrap().outcome.is_none());
    let events = fx.store.events().unwrap();
    let delivered = events
        .iter()
        .rfind(|e| e.kind == "changes_requested")
        .unwrap();
    assert_eq!(delivered.detail["via"], "agent_prompt");
    assert_eq!(delivered.detail["submitted"], true);
    assert_eq!(delivered.detail["attempts"], 1);
}

#[test]
fn never_visible_paste_remains_pending_with_zero_submission_attempts() {
    // Given a fake herdr that accepts a paste but never displays it.
    let fx = Fixture::new();
    std::fs::write(
        &fx.herdr,
        r#"#!/bin/sh
dir="$(dirname "$0")"
if [ "$1" = --session ]; then shift 2; fi
case "$1 $2" in
  "agent prompt") printf '%s\n' '{"error":{"code":"agent_not_ready","message":"busy"}}'; exit 1 ;;
  "pane read") printf '❯ \n' ;;
  "pane send-text") printf '%s' "$4" > "$dir/pasted"; printf '%s\n' '{"result":{}}' ;;
  "pane send-keys") touch "$dir/enter"; printf '%s\n' '{"result":{}}' ;;
  *) exit 1 ;;
esac
"#,
    )
    .unwrap();
    let ctx = corral_ops::Ctx {
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        store: fx.store.clone(),
        herdr: corral_herdr::Herdr::with_bin(fx.herdr.clone(), None).with_poll_wait(|_| {}),
    };
    // When the bounded poller exhausts its fake clock without seeing the input.
    let error = corral_ops::reply(&ctx, "T-001", "must not disappear").unwrap_err();
    // Then the reopened store, event, and visible error all report failure, without Enter.
    assert!(format!("{error:#}").contains("reply not delivered"));
    let reopened = Store::open(fx.root.path()).unwrap();
    let task = reopened.get("T-001").unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(
        task.current_run().unwrap().pending_reply.as_deref(),
        Some("must not disappear")
    );
    let events = reopened.events().unwrap();
    let delivery = events.iter().find(|e| e.kind == "replied").unwrap();
    assert_eq!(delivery.detail["via"], "send_text");
    assert_eq!(delivery.detail["submitted"], false);
    assert_eq!(delivery.detail["attempts"], 0);
    assert!(!fx.root.path().join("enter").exists());
}
