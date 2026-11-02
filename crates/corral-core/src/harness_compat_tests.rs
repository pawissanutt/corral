use super::*;

#[test]
fn legacy_harness_tables_and_history_round_trip_canonically() {
    let config: crate::Config = toml::from_str(
        r#"
[harnesses.pi-native]
command = "{corral} agent launch {delivery_args}"
flags = "pi-native"
[profiles.pi-sol]
harness = "pi-native"
"#,
    )
    .unwrap();
    assert!(config.harnesses.contains_key("corral-pi"));
    assert!(!config.to_toml().contains("pi-native"));
    assert_eq!(
        config
            .resolve_profile(
                &crate::ProfileChoice {
                    profile: Some("pi-sol".into()),
                    ..Default::default()
                },
                None,
                None
            )
            .unwrap()
            .harness,
        "corral-pi"
    );

    let event: crate::Event = serde_json::from_value(serde_json::json!({
        "ts": "2026-11-01T00:00:00Z", "kind": "dispatched", "via": "pi_native",
        "detail": {"harness": "pi-native", "nested": [{"adapter": "pi_native"}],
            "profile": "pi-native", "note": "pi-native"}
    }))
    .unwrap();
    assert_eq!(event.via.as_deref(), Some("corral_pi"));
    assert_eq!(event.detail["harness"], "corral-pi");
    assert_eq!(event.detail["nested"][0]["adapter"], "corral_pi");
    // Names and historical free text are not harness identifiers.
    assert_eq!(event.detail["profile"], "pi-native");
    assert_eq!(event.detail["note"], "pi-native");
    let encoded = serde_json::to_value(event).unwrap();
    assert_eq!(encoded["detail"]["harness"], "corral-pi");
}

#[test]
fn legacy_installed_data_and_skills_are_read_in_place_and_never_moved() {
    let tmp = tempfile::tempdir().unwrap();
    for parent in [tmp.path().join("corral"), tmp.path().join(".corral/skills")] {
        let legacy = parent.join("pi-native");
        let target = parent.join("corral-pi");
        assert_eq!(directory(&target), target);
        assert!(!parent.exists());
        std::fs::create_dir_all(legacy.join("auth")).unwrap();
        std::fs::write(legacy.join("auth/credential"), "retained").unwrap();
        assert_eq!(directory(&target), legacy);
        assert_eq!(
            std::fs::read_to_string(legacy.join("auth/credential")).unwrap(),
            "retained"
        );
        assert!(!target.exists());
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("other"), "new install").unwrap();
        assert_eq!(directory(&target), target);
        assert!(legacy.join("auth/credential").exists());
        assert!(!target.join("auth").exists());
    }
}
