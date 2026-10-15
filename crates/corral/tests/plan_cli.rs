use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::{Status, Store};
use serde_json::Value;

const PLAN: &str = "---\ngoal: CLI fixture\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance:\n  - scenario: widget --spin\n    expected: clockwise\nverification: [cargo test]\nowner_decisions: []\n---\n";

fn cli(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CORRAL_")
            || name.to_string_lossy().starts_with("HERDR_")
        {
            command.env_remove(name);
        }
    }
    command
        .env("CORRAL_ROOT", root)
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn json(root: &Path, args: &[&str]) -> Value {
    let out = cli(root, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn plan_cli_preserves_history_and_never_accepts_implementation() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("Plan CLI fixture", None, Status::Ready, "Widget")
        .unwrap();
    let path = tmp.path().join("plan.md");
    let notes = tmp.path().join("notes.md");
    std::fs::write(&path, PLAN).unwrap();
    std::fs::write(&notes, "Manual owner approval; no remaining decisions").unwrap();
    let required = json(tmp.path(), &["plan", "policy", &task.id, "required"]);
    assert_eq!(required["plan_required"], true);
    let plan = json(
        tmp.path(),
        &["plan", "submit", &task.id, path.to_str().unwrap(), "--json"],
    );
    assert_eq!(plan["id"], "P1");
    let result = json(
        tmp.path(),
        &[
            "plan",
            "verdict",
            &task.id,
            "approve",
            "--plan",
            "P1",
            "--notes-file",
            notes.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(result["verdict"], "approve");
    let shown = json(tmp.path(), &["plan", "show", &task.id, "--json"]);
    assert_eq!(
        shown["plans"][0]["artifact"]["sha256"],
        plan["artifact"]["sha256"]
    );
    assert_eq!(shown["plans"][0]["reviews"][0]["reviewer"], "owner");
    assert_eq!(store.get(&task.id).unwrap().status, Status::Ready);
    assert!(store.get(&task.id).unwrap().runs.is_empty());
    assert!(
        !cli(
            tmp.path(),
            &[
                "verdict",
                &task.id,
                "approve",
                "--notes-file",
                notes.to_str().unwrap()
            ]
        )
        .status
        .success()
    );
    assert!(!cli(tmp.path(), &["approve", &task.id]).status.success());
    assert_eq!(store.list().unwrap().len(), 1);
}

#[test]
fn plan_verdict_requires_explicit_target_and_rejects_replaced_target() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("Plan identity", None, Status::Ready, "")
        .unwrap();
    let path = tmp.path().join("plan.md");
    let notes = tmp.path().join("notes.md");
    std::fs::write(&path, PLAN).unwrap();
    std::fs::write(&notes, "Old verdict").unwrap();
    for _ in 0..2 {
        json(
            tmp.path(),
            &["plan", "submit", &task.id, path.to_str().unwrap(), "--json"],
        );
    }
    assert!(
        !cli(
            tmp.path(),
            &[
                "plan",
                "verdict",
                &task.id,
                "approve",
                "--notes-file",
                notes.to_str().unwrap()
            ]
        )
        .status
        .success()
    );
    let stale = cli(
        tmp.path(),
        &[
            "plan",
            "verdict",
            &task.id,
            "approve",
            "--plan",
            "P1",
            "--notes-file",
            notes.to_str().unwrap(),
        ],
    );
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("replaced"));
    let shown = json(tmp.path(), &["plan", "show", &task.id, "--json"]);
    assert_eq!(shown["plans"].as_array().unwrap().len(), 2);
    assert!(shown["plans"][1].get("reviews").is_none());
}
