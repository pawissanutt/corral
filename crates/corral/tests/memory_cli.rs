//! Operator CLI against disposable workspace Git; no providers or live herdr.
use corral_core::Store;
use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Command, Output},
};

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_corral"))
        .arg("--root")
        .arg(root)
        .arg("memory")
        .args(args)
        .current_dir(root)
        .env_remove("CORRAL_ASSISTANT")
        .env_remove("CORRAL_PM")
        .env_remove("CORRAL_TASK")
        .env_remove("CORRAL_AGENT_BOOTSTRAP")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}
fn ok(root: &Path, args: &[&str]) -> Value {
    let output = cli(root, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
fn entry() -> Value {
    json!({"metadata":{"version":1,"id":"preferences","scope":{"scope":"workspace"},
    "title":"Preferences","kind":"preference","summary":"Be concise",
    "provenance":{"kind":"imported_reference","sources":["fixture:notes"]},
    "updated_at":"2026-01-01T00:00:00Z","author":"forged"},"body":"Longer detail"})
}
#[test]
fn memory_cli_cas_pagination_and_git_are_explicit() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    Store::init(root).unwrap();
    git(root, &["init"]);
    std::fs::write(root.join("unrelated"), "committed\n").unwrap();
    git(root, &["add", "unrelated"]);
    git(root, &["commit", "-m", "fixture"]);
    std::fs::write(root.join("unrelated"), "staged\n").unwrap();
    git(root, &["add", "unrelated"]);
    std::fs::write(root.join("unrelated"), "unstaged\n").unwrap();
    let head = git(root, &["rev-parse", "HEAD"]);
    let index = std::fs::read(root.join(".git/index")).unwrap();
    let input = root.join("input.json");
    std::fs::write(&input, entry().to_string()).unwrap();
    let file = input.to_str().unwrap();
    let created = ok(
        root,
        &[
            "--scope",
            "workspace",
            "put",
            "preferences",
            "--create",
            "--file",
            file,
        ],
    );
    assert_eq!(created["metadata"]["author"], "owner");
    let hash = created["hash"].as_str().unwrap();
    assert_eq!(
        ok(root, &["--scope", "workspace", "list"])["entries"][0]["hash"],
        hash
    );
    let read = ok(
        root,
        &[
            "--scope",
            "workspace",
            "read",
            "preferences",
            "--limit",
            "6",
        ],
    );
    assert_eq!(read["body"], "Longer");
    assert_eq!(read["next_offset"], 6);
    assert!(
        !cli(
            root,
            &[
                "--scope",
                "workspace",
                "put",
                "preferences",
                "--create",
                "--file",
                file
            ]
        )
        .status
        .success()
    );
    let mut updated = entry();
    updated["body"] = json!("replacement");
    std::fs::write(&input, updated.to_string()).unwrap();
    let changed = ok(
        root,
        &[
            "--scope",
            "workspace",
            "put",
            "preferences",
            "--expected-hash",
            hash,
            "--file",
            file,
        ],
    );
    assert!(
        !cli(
            root,
            &[
                "--scope",
                "workspace",
                "remove",
                "preferences",
                "--expected-hash",
                hash
            ]
        )
        .status
        .success()
    );
    ok(
        root,
        &[
            "--scope",
            "workspace",
            "remove",
            "preferences",
            "--expected-hash",
            changed["hash"].as_str().unwrap(),
        ],
    );
    assert!(ok(root, &["--scope", "workspace", "read", "preferences"]).is_null());
    assert_eq!(git(root, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(
        std::fs::read_to_string(root.join("unrelated")).unwrap(),
        "unstaged\n"
    );
    assert_eq!(git(root, &["show", ":unrelated"]), b"staged\n");
}

#[test]
fn memory_cli_bound_shell_has_no_operator_bypass() {
    let tmp = tempfile::tempdir().unwrap();
    Store::init(tmp.path()).unwrap();
    for (key, value) in [
        ("CORRAL_ASSISTANT", "A-001"),
        ("CORRAL_PM", "1"),
        ("CORRAL_TASK", "T-001"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_corral"))
            .arg("--root")
            .arg(tmp.path())
            .args(["memory", "--scope", "workspace", "list"])
            .env(key, value)
            .current_dir(tmp.path())
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success(), "{key}");
    }
}
