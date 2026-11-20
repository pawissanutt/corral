use std::{fs, os::unix::fs::PermissionsExt};

use corral_protocol::{rows::SettingsSnapshot, subscription::Topic};

use super::{HarnessStatus, PiConfigStatus, literal_program, locate};
use crate::views::{Rows, tests::Fixture};

fn settings(fixture: &Fixture) -> SettingsSnapshot {
    let shared = &fixture.shared;
    let rows = shared.row_cache.get(shared, Topic::Settings).unwrap();
    let Rows::Settings(rows) = rows.as_ref() else {
        panic!("expected Settings rows");
    };
    rows[0].clone()
}

#[test]
fn settings_use_cached_host_facts_without_leaking_pi_settings_or_reusing_an_old_command() {
    let fixture = Fixture::new();
    let shared = &fixture.shared;
    let state = shared.root().join(crate::STATE_DIR);
    let executable = state.join("custom-exe");
    let marker = state.join("must-not-run");
    fs::write(
        &executable,
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let config = state.join("corral.toml");
    fs::write(
        &config,
        format!(
            "[harnesses.custom]\ncommand = {:?}\n",
            format!("{} --header example-argument", executable.display())
        ),
    )
    .unwrap();
    let pi = state.join("pi.toml");
    fs::write(
        &pi,
        r#"
[build.profiles.reviewer]
model = "anthropic-api/claude-opus-5-5"
effort = "high"
description = "description-not-served"
api_key = "credential-not-served"
[web]
api_key = "another-credential-not-served"
"#,
    )
    .unwrap();

    let before = settings(&fixture);
    assert!(before.harness_facts.is_none() && before.pi_config.is_none());
    shared.harness_inventory.refresh(shared);
    let observed = settings(&fixture);
    let facts = observed.harness_facts.as_ref().unwrap();
    let custom = facts.iter().find(|fact| fact.name == "custom").unwrap();
    assert_eq!(custom.status, HarnessStatus::Installed);
    assert_eq!(custom.executable.as_deref(), executable.to_str());
    assert_eq!(custom.resolved_path.as_deref(), executable.to_str());
    let pi_facts = observed.pi_config.as_ref().unwrap();
    assert_eq!(pi_facts.path, pi.to_str().unwrap());
    assert_eq!(pi_facts.status, PiConfigStatus::Read);
    assert_eq!(
        serde_json::to_value(&pi_facts.child_profiles).unwrap(),
        serde_json::json!([
            {"name":"reviewer", "model":"anthropic-api/claude-opus-5-5", "effort":"high"}
        ])
    );
    let projected = serde_json::to_string(&(facts, pi_facts)).unwrap();
    assert!(!projected.contains("not-served") && !projected.contains("example-argument"));
    assert!(!marker.exists());

    fs::remove_file(&executable).unwrap();
    fs::remove_file(&pi).unwrap();
    // Even when the row cache is invalidated, a read cannot update host facts itself.
    shared.publish();
    assert_eq!(settings(&fixture), observed);

    fs::write(&config, "[harnesses.custom]\ncommand = 'new-command'\n").unwrap();
    shared.refresh_config().unwrap();
    assert!(settings(&fixture).harness_facts.is_none());
    shared.harness_inventory.refresh(shared);
    assert_eq!(
        settings(&fixture).pi_config.unwrap().status,
        PiConfigStatus::Missing
    );
    fs::write(
        &pi,
        "[build.profiles.legacy]\nmodel = 'anthropic/opus'\nconnection = 'anthropic-api'\n",
    )
    .unwrap();
    shared.harness_inventory.refresh(shared);
    let legacy = settings(&fixture).pi_config.unwrap();
    assert_eq!(legacy.status, PiConfigStatus::Unavailable);
    assert!(legacy.child_profiles.is_empty());
}

#[test]
fn executable_lookup_requires_an_observation_not_a_shell_guess() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("demo");
    let search = Some(dir.path().as_os_str());
    fs::write(&exe, "").unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(locate("demo", search), (HarnessStatus::NotFound, None));
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        locate("demo", search),
        (HarnessStatus::Installed, exe.to_str().map(str::to_owned))
    );
    assert_eq!(locate("demo", None), (HarnessStatus::Unknown, None));
    assert_eq!(locate("./demo", search), (HarnessStatus::Unknown, None));
    assert_eq!(literal_program("API_KEY=not-for-display demo"), None);
    assert_eq!(literal_program("${HARNESS} run"), None);
    assert_eq!(literal_program("env demo"), None);
    assert_eq!(
        literal_program("\"corral-pi\" run {model_args}"),
        Some("corral-pi")
    );
}
