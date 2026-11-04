//! Machine-readable repair errors identify the repair operation, never setup.
use std::process::{Command, Stdio};

#[test]
fn repair_failure_keeps_its_command_identity() {
    // No initialized store is needed to exercise the shared CLI error boundary.
    let root = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_corral"))
        .env_clear()
        .env("HOME", root.path())
        .args(["--root"])
        .arg(root.path())
        .args(["repair", "status", "T-001"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["command"], "repair status");
    assert!(error["error"].as_str().is_some_and(|text| !text.is_empty()));
}
