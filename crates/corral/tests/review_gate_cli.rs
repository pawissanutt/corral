//! Gate configuration failures are observable before a terminal process is launched.
use std::process::{Command, Stdio};

use corral_core::{Status, Store};

fn command(root: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    command
        .arg("--root")
        .arg(root)
        .env("CORRAL_ROOT", root)
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .env_remove("CORRAL_GATE_ATTEMPT")
        .stdin(Stdio::null());
    command
}

#[test]
fn gate_cli_exits_nonzero_with_project_error_when_effective_commands_are_empty() {
    // Given a project with an explicit review policy but no gate.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("app/.git")).unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.review.accept = Some(corral_core::Accept::Owner);
    store.save_config().unwrap();
    let task = store
        .create("No gate", Some("app"), Status::Ready, "")
        .unwrap();

    // When the actual CLI is invoked, without an engine or run to hide configuration errors.
    let output = command(tmp.path())
        .args(["gate", &task.id])
        .output()
        .unwrap();

    // Then the CLI refuses verification and reports the actionable project identity.
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("project app has no gate configured"),
        "{output:?}"
    );
}

#[test]
fn config_check_cli_accepts_gate_environment_and_rejects_invalid_names() {
    // Given a valid per-project env table.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("app/.git")).unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.projects.get_mut("app").unwrap().review.env =
        Some([("CORRAL_HERDR_SESSION".into(), "corral-test".into())].into());
    store.save_config().unwrap();
    let valid = command(tmp.path())
        .args(["config", "check"])
        .output()
        .unwrap();
    assert!(valid.status.success(), "{valid:?}");

    // When the environment table contains a name the shell cannot export.
    let text = std::fs::read_to_string(store.config_path()).unwrap();
    std::fs::write(
        store.config_path(),
        text.replace("CORRAL_HERDR_SESSION", "BAD-NAME"),
    )
    .unwrap();
    let invalid = command(tmp.path())
        .args(["config", "check"])
        .output()
        .unwrap();

    // Then config check refuses the invalid execution input.
    assert!(!invalid.status.success());
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("BAD-NAME"),
        "{invalid:?}"
    );
}

#[test]
fn doctor_cli_ignores_the_deprecated_max_diff_lines_key_when_judging_review_policy() {
    // Given a project whose only review setting is the removed max_diff_lines key.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("app/.git")).unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut text = std::fs::read_to_string(store.config_path()).unwrap();
    text.push_str("\n[review]\nmax_diff_lines = 1500\n");
    std::fs::write(store.config_path(), text).unwrap();

    // When doctor inspects it via the real JSON CLI.
    let output = command(tmp.path())
        .env("HOME", tmp.path())
        .env("PATH", "")
        .env("HERDR_BIN_PATH", tmp.path().join("absent-herdr"))
        .args(["doctor", "--json"])
        .output()
        .unwrap();

    // Then the key loads (with its deprecation warning) but demands no gate.
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        !report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["id"] == "review_gate"),
        "{report}"
    );
}

#[test]
fn doctor_cli_lists_missing_project_gate_and_repository_entry_point() {
    // Given a configured review policy and a repo-owned script, without external tools.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("app/.git")).unwrap();
    std::fs::create_dir_all(tmp.path().join("app/scripts")).unwrap();
    std::fs::write(
        tmp.path().join("app/scripts/gate.sh"),
        "#!/bin/sh\nexit 0\n",
    )
    .unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.review.accept = Some(corral_core::Accept::Owner);
    store.save_config().unwrap();

    // When doctor inspects the project via its real JSON CLI.
    let output = command(tmp.path())
        .env("HOME", tmp.path())
        .env("PATH", "")
        .env("HERDR_BIN_PATH", tmp.path().join("absent-herdr"))
        .args(["doctor", "--json"])
        .output()
        .unwrap();

    // Then the project-specific warning carries a suggested versioned entry point.
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let gate = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "review_gate")
        .unwrap();
    assert_eq!(gate["ok"], false);
    assert_eq!(gate["message"], "project app has no gate configured");
    assert!(
        gate["fix"]
            .as_str()
            .unwrap()
            .contains("bash scripts/gate.sh")
    );
    assert!(
        Store::open(tmp.path())
            .unwrap()
            .config
            .review
            .gate
            .is_none()
    );
}
