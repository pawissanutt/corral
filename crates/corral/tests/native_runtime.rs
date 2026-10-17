//! Native CLI contracts; no live engine, shell fixtures, or agent session.
use std::process::{Command, Stdio};

use corral_ops::{ProcessControl, SystemProcesses};

#[test]
fn native_runtime_probe_matches_process_identity_and_monotonic_clock() {
    // Given this live native process and the actual CLI binary.
    let identity = SystemProcesses
        .inspect(std::process::id())
        .unwrap()
        .unwrap()
        .started;
    let probe = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_corral"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    // When the shell-facing CLI captures identity between clock readings.
    let before: u64 = probe(&["runtime-probe"]).parse().unwrap();
    let actual = probe(&["runtime-probe", "--pid", &std::process::id().to_string()]);
    let after: u64 = probe(&["runtime-probe"]).parse().unwrap();
    // Then the CLI protocol is the same native identity/clock used by gates.
    assert_eq!(actual, identity);
    assert!(after >= before);
}

#[test]
fn native_plugin_manifest_advertises_current_version_and_supported_platforms() {
    // Given a private store and no engine binary or engine config.
    let root = tempfile::tempdir().unwrap();
    let _store = corral_core::Store::init(root.path()).unwrap();
    let config = root.path().join("engine-config.toml");
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for key in [
        "CORRAL_GATE_ATTEMPT",
        "CORRAL_CANDIDATE_HEAD",
        "CORRAL_INTEGRATION_BASE",
        "HERDR_PANE_ID",
        "HERDR_WORKSPACE_ID",
        "HERDR_TAB_ID",
    ] {
        command.env_remove(key);
    }
    // When the native CLI renders the bundled plugin without linking it.
    let output = command
        .args(["plugin", "install", "--print"])
        .arg("--root")
        .arg(root.path())
        .env("HERDR_CONFIG_PATH", &config)
        .env("HERDR_BIN_PATH", root.path().join("missing-engine"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = std::fs::read_to_string(
        root.path()
            .join(".corral/state/herdr-plugin/herdr-plugin.toml"),
    )
    .unwrap();
    let manifest: toml::Value = toml::from_str(&text).unwrap();
    // Then the engine receives machine-consumed platform/version values for this release.
    assert_eq!(
        manifest["version"].as_str(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    let platforms = manifest["platforms"].as_array().unwrap();
    for platform in ["linux", "macos", "windows"] {
        assert!(
            platforms
                .iter()
                .any(|value| value.as_str() == Some(platform))
        );
    }
    assert!(!config.exists());
}
