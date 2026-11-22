#![cfg(unix)]

use corral_engine::{Engine, NewTask, Op, Role, Run};
use serde_json::Value;
use std::{fs, process::Command};

#[test]
fn tool_lists_and_exchanges_request_in_callers_directory() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join(".corral-next");
    fs::create_dir(&state).unwrap();
    fs::write(root.path().join("fixture.sh"), "#!/bin/sh\ncat\n").unwrap();
    fs::write(
        state.join("corral.toml"),
        r#"
[projects.site]
path = "."
[extensions.fixture.tools]
context = { run = "sh fixture.sh", about = "Find context" }
"#,
    )
    .unwrap();
    let call = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_corral-next"))
            .arg("--root")
            .arg(root.path())
            .args(args)
            .current_dir(root.path())
            .env_remove("CORRAL_TASK")
            .env_remove("CORRAL_RUN")
            .env_remove("CORRAL_ACTOR")
            .env_remove("CORRAL_PROJECT")
            .output()
            .unwrap()
    };
    let list = call(&["tool"]);
    assert!(list.status.success(), "{list:?}");
    let text = String::from_utf8(list.stdout).unwrap();
    assert!(text.contains("context") && text.contains("Find context"));
    let output = call(&["tool", "context", "--example", "value"]);
    assert!(output.status.success(), "{output:?}");
    let request: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(request["v"], 1);
    assert_eq!(request["args"], serde_json::json!(["--example", "value"]));
    assert_eq!(request.get("head"), Some(&Value::Null));

    // A session targeting another workspace must not import its run identity.
    let other = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_corral-next"))
        .arg("--root")
        .arg(root.path())
        .args(["tool", "context"])
        .current_dir(root.path())
        .env("CORRAL_NEXT_ROOT", other.path())
        .env("CORRAL_TASK", "T-999")
        .env("CORRAL_RUN", "R1")
        .env("CORRAL_PROJECT", "other")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let request: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(request["task"].is_null());
    assert!(request["run"].is_null());
}

#[test]
fn tool_receives_submitted_head_after_worktree_moves() {
    let root = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(root.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "--allow-empty",
        "-qm",
        "base",
    ]);
    let base = git(&["rev-parse", "HEAD"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "--allow-empty",
        "-qm",
        "submitted",
    ]);
    let submitted = git(&["rev-parse", "HEAD"]);
    let engine = Engine::init(root.path(), None).unwrap();
    fs::write(
        engine.state_dir.join("corral.toml"),
        "[extensions.fixture.tools]\necho = { run = 'cat' }\n",
    )
    .unwrap();
    let task = engine
        .apply(
            Op::New {
                task: NewTask {
                    title: "fixture".into(),
                    ..NewTask::default()
                },
            },
            false,
        )
        .unwrap()
        .task
        .unwrap();
    engine
        .store
        .insert_run(&Run {
            id: "R1".into(),
            task: task.id.clone(),
            role: Role::Worker,
            profile: "generic".into(),
            harness: "generic".into(),
            model: None,
            effort: None,
            worktree: root.path().into(),
            branch: "task".into(),
            base: "main".into(),
            base_sha: base.clone(),
            session: None,
            started: jiff::Timestamp::now(),
            ended: None,
            outcome: None,
            summary: None,
            head_sha: Some(submitted.clone()),
            quiet_since: None,
        })
        .unwrap();
    let mut reviewer = engine.store.get_run(&task.id, "R1").unwrap().unwrap();
    reviewer.id = "R2".into();
    reviewer.role = Role::Reviewer;
    reviewer.head_sha = None;
    engine.store.insert_run(&reviewer).unwrap();
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "--allow-empty",
        "-qm",
        "later",
    ]);
    assert_ne!(git(&["rev-parse", "HEAD"]), submitted);
    let call = || {
        let output = Command::new(env!("CARGO_BIN_EXE_corral-next"))
            .arg("--root")
            .arg(root.path())
            .args(["tool", "echo"])
            .current_dir(root.path())
            .env("CORRAL_NEXT_ROOT", root.path())
            .env("CORRAL_TASK", &task.id)
            .env("CORRAL_RUN", "R2")
            .env("CORRAL_ACTOR", "reviewer")
            .env_remove("CORRAL_PROJECT")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let request: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(request["v"], 1);
        assert_eq!(request["base"], base);
        assert_eq!(request["head"], submitted);
    };
    call();
    // Reused reviewers retain the previous verdict's head as a diff baseline.
    reviewer.head_sha = Some(base.clone());
    engine.store.update_run(&reviewer).unwrap();
    call();
}
