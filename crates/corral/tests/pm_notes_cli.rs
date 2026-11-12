//! Notes CLI mutations preserve the caller's owner/PM distinction.
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn call(root: &Path, caller: &[(&str, &str)], args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_corral"))
        .env_clear()
        .env("HOME", root.join("home"))
        .env("PATH", "/nonexistent")
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .envs(caller.iter().copied())
        .arg("--root")
        .arg(root)
        .args(["pm", "notes"])
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn committed(root: &Path, caller: &[(&str, &str)], args: &[&str]) -> Value {
    let output = call(root, caller, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn notes_cli_preserves_pm_identity_and_owner_entry_protection() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("home")).unwrap();
    corral_core::Store::init(root).unwrap();
    let pm = &[("CORRAL_PM", "1")];
    let owner = committed(
        root,
        &[],
        &[
            "add",
            "--kind",
            "instruction",
            "--text",
            "owner context",
            "--json",
        ],
    );
    assert_eq!(owner["entries"][0]["author"], "owner");
    let added = committed(
        root,
        pm,
        &[
            "add",
            "--kind",
            "hypothesis",
            "--text",
            "PM context",
            "--json",
        ],
    );
    assert_eq!(added["entries"][1]["author"], "pm");
    assert_eq!(added["revision"], 2);

    for args in [
        vec!["remove", "n-1"],
        vec!["supersede", "n-1", "--text", "replacement"],
        vec![
            "add",
            "--kind",
            "instruction",
            "--text",
            "uncited instruction",
        ],
    ] {
        assert!(!call(root, pm, &args).status.success());
        assert_eq!(committed(root, &[], &["list", "--json"]), added);
    }
    assert!(
        !call(root, &[("CORRAL_AGENT_TASK", "T-1")], &["remove", "n-1"])
            .status
            .success()
    );
    assert_eq!(committed(root, &[], &["list", "--json"]), added);

    let replaced = committed(
        root,
        &[],
        &["supersede", "n-1", "--text", "owner replacement", "--json"],
    );
    assert_eq!(replaced["entries"][2]["author"], "owner");
    let removed = committed(root, &[], &["remove", "n-3", "--json"]);
    assert_eq!(removed["revision"], 4);
    assert_eq!(removed["entries"].as_array().unwrap().len(), 2);
}
