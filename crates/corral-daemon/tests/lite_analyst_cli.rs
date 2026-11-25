#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::process::{Command, Stdio};

#[test]
fn fixture_subcommand_requires_opt_in_before_workspace_or_descriptor_access() {
    let scratch = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_corrald-next"))
        .arg("lite-analyst-fixture-v1")
        .env_clear()
        .current_dir(scratch.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("explicit per-launch opt-in"));
    assert_eq!(scratch.path().read_dir().unwrap().count(), 0);
}
