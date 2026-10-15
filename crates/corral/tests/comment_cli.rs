use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::{CommentAuthor, Status, Store};
use serde_json::{Value, json};

struct Fixture {
    root: tempfile::TempDir,
    store: Store,
}

impl Fixture {
    fn new(status: Status) -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        store.create("Comments", None, status, "Task body").unwrap();
        Self { root, store }
    }

    fn cli(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
                command.env_remove(name);
            }
        }
        command
            .current_dir(self.root.path())
            .env("CORRAL_ROOT", self.root.path())
            .env("CORRAL_HERDR_SESSION", "corral-test")
            // These tests never need a terminal engine, even if one is running.
            .env("HERDR_BIN_PATH", self.root.path().join("no-herdr"))
            .arg("--root")
            .arg(self.root.path())
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.cli(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn show_json(&self) -> Value {
        serde_json::from_str(&self.ok(&["show", "T-001", "--json"])).unwrap()
    }
}

fn reopened(root: &Path) -> Store {
    Store::open(root).unwrap()
}

#[test]
fn default_owner_comment_persists_and_emits_event() {
    // Given an ordinary backlog task.
    let fx = Fixture::new(Status::Backlog);
    let events_before = fx.store.events().unwrap().len();
    let body_before = fx.store.get("T-001").unwrap().body;
    let text = "First line\n\nSecond line: \"quoted\"";
    // When the CLI appends a comment without an explicit author.
    fx.ok(&["comment", "1", text]);
    // Then a fresh store and CLI process observe the same structured comment.
    let store = reopened(fx.root.path());
    let task = store.get("T-001").unwrap();
    assert_eq!(task.comments.len(), 1);
    assert_eq!(task.comments[0].author, CommentAuthor::Owner);
    assert_eq!(task.comments[0].text, text);
    assert_eq!(task.body, body_before);
    assert_eq!(task.status, Status::Backlog);
    assert_eq!(
        fx.show_json()["comments"],
        json!([{
            "author": "owner",
            "at": task.comments[0].at,
            "text": text
        }])
    );
    let events = store.events().unwrap();
    assert_eq!(events.len(), events_before + 1);
    let event = events.last().unwrap();
    assert_eq!(event.task.as_deref(), Some("T-001"));
    assert_eq!(event.kind, "commented");
    assert_eq!(event.detail["comment"]["author"], "owner");
    assert_eq!(event.detail["comment"]["text"], text);
    assert_eq!(event.detail["send"], false);
    let human = fx.ok(&["show", "T-001"]);
    assert!(human.contains("owner"));
    assert!(human.contains(&task.comments[0].at.to_string()));
    assert!(human.contains(text));
}

#[test]
fn pm_comment_appends_after_owner_and_is_visible_in_show() {
    // Given a task with an existing owner comment.
    let fx = Fixture::new(Status::Ready);
    fx.ok(&["comment", "T-001", "Owner context"]);
    // When the PM appends a separate comment.
    fx.ok(&["comment", "T-001", "PM context", "--author", "pm"]);
    // Then both comments survive in order with their own authors.
    let task = reopened(fx.root.path()).get("T-001").unwrap();
    assert_eq!(task.comments.len(), 2);
    assert_eq!(task.comments[0].author, CommentAuthor::Owner);
    assert_eq!(task.comments[1].author, CommentAuthor::Pm);
    assert_eq!(task.comments[1].text, "PM context");
    assert_eq!(fx.show_json()["comments"][1]["author"], "pm");
    let human = fx.ok(&["show", "T-001"]);
    assert!(human.contains("pm"));
    assert!(human.contains(&task.comments[1].at.to_string()));
    assert!(human.contains("PM context"));
}

#[test]
fn show_json_includes_empty_comments() {
    // Given a task without comments.
    let fx = Fixture::new(Status::Idea);
    // When the task is shown as JSON.
    let shown = fx.show_json();
    // Then consumers always receive an array, not a missing field or null.
    assert_eq!(shown["comments"], json!([]));
}

#[test]
fn comment_is_status_neutral_across_all_task_statuses() {
    for status in [
        Status::Idea,
        Status::Backlog,
        Status::Ready,
        Status::Running,
        Status::NeedsMe,
        Status::Review,
        Status::Done,
        Status::Dropped,
    ] {
        // Given a task in each lifecycle state, without a running agent.
        let fx = Fixture::new(status);
        // When a comment is recorded without --send.
        fx.ok(&[
            "comment",
            "T-001",
            "Status-independent note",
            "--author",
            "owner",
        ]);
        // Then no engine is required and the status stays intact.
        let task = reopened(fx.root.path()).get("T-001").unwrap();
        assert_eq!(task.status, status);
        assert_eq!(task.comments.len(), 1);
        assert_eq!(task.comments[0].text, "Status-independent note");
    }
}

#[test]
fn send_requires_running_agent_without_recording_comment() {
    // Given a task that has no running agent.
    let fx = Fixture::new(Status::Backlog);
    let before = fx.store.events().unwrap();
    // When --send requests delivery as well as persistence.
    let output = fx.cli(&["comment", "T-001", "Send this", "--send"]);
    // Then the operation rejects delivery without an engine or partial mutation.
    assert!(!output.status.success());
    assert!(!output.stderr.is_empty());
    let store = reopened(fx.root.path());
    assert!(store.get("T-001").unwrap().comments.is_empty());
    assert_eq!(store.events().unwrap(), before);
}

#[test]
fn empty_and_invalid_comment_arguments_do_not_mutate_task() {
    let invalid: &[&[&str]] = &[
        &["comment"],
        &["comment", "T-001"],
        &["comment", "T-001", ""],
        &["comment", "T-001", " \t\n"],
        &["comment", "T-001", "note", "--author", "agent"],
        &["comment", "T-001", "note", "--author"],
        &["comment", "T-001", "note", "--unknown"],
        &["comment", "T-001", "note", "extra"],
        &["comment", "invalid", "note"],
        &["comment", "T-999", "note"],
    ];
    for args in invalid {
        // Given a task and its event log.
        let fx = Fixture::new(Status::Backlog);
        let before = fx.store.events().unwrap();
        // When malformed, empty, or unresolvable input is submitted.
        let output = fx.cli(args);
        // Then failure is visible and neither task comments nor events change.
        assert!(!output.status.success(), "{args:?}");
        assert!(!output.stderr.is_empty(), "{args:?}");
        let store = reopened(fx.root.path());
        assert!(store.get("T-001").unwrap().comments.is_empty());
        assert_eq!(store.events().unwrap(), before);
    }
}
