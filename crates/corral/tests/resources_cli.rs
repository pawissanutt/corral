//! Config checking does not probe the machine or require a terminal engine.
use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::{ConfigDoc, Store};

fn check(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_corral"))
        .env("CORRAL_ROOT", root)
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .args(["config", "check"])
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn config_check_accepts_resource_defaults_and_partial_overrides() {
    // Given an isolated store and a partial resources table.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut config = ConfigDoc::load(&store.config_path()).unwrap();
    config.unset("resources.max_load_per_core").unwrap();
    config.unset("resources.min_mem_available").unwrap();
    config.set_str("resources.min_disk_free", "'2GB'").unwrap();
    std::fs::write(store.config_path(), config.to_string()).unwrap();
    // When the real CLI validates it through the requested check alias.
    let output = check(tmp.path());
    // Then omitted thresholds use valid defaults.
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn config_check_rejects_nonpositive_and_nonfinite_resource_loads() {
    for load in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        // Given a hand-edited invalid load threshold in an isolated config.
        let tmp = tempfile::tempdir().unwrap();
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.resources.max_load_per_core = load;
        std::fs::write(store.config_path(), store.config.to_toml()).unwrap();
        // When the real CLI validates the file.
        let output = check(tmp.path());
        // Then it rejects the invalid resources field, without probing the host.
        assert!(!output.status.success(), "load {load}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("resources.max_load_per_core"),
            "{output:?}"
        );
    }
}

#[test]
fn config_check_rejects_invalid_resource_byte_floors() {
    for (key, value) in [
        ("min_mem_available", "0GB"),
        ("min_mem_available", "-8GB"),
        ("min_disk_free", "nonsense"),
        ("min_disk_free", "18446744073709551615GB"),
    ] {
        // Given a hand-edited invalid byte floor.
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path()).unwrap();
        let mut config = ConfigDoc::load(&store.config_path()).unwrap();
        config
            .set_str(&format!("resources.{key}"), &format!("'{value}'"))
            .unwrap();
        std::fs::write(store.config_path(), config.to_string()).unwrap();
        // When the real CLI checks the file.
        let output = check(tmp.path());
        // Then the invalid field causes failure.
        assert!(!output.status.success(), "{key} = {value}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(&format!("resources.{key}")),
            "{output:?}"
        );
    }
}
