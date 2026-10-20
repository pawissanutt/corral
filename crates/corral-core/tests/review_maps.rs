use std::collections::BTreeMap;

use corral_core::{Accept, Config, PolicySource};

#[test]
fn configured_policy_distinguishes_explicit_manual_settings_from_defaults() {
    let config: Config = toml::from_str(
        "[projects.app]\npath = \".\"\n[projects.app.review]\nremove_worktree = true",
    )
    .unwrap();
    assert!(!config.has_review_policy(None, None));
    assert!(!config.has_review_policy(Some("unknown"), None));
    assert!(config.has_review_policy(Some("app"), None));
    assert!(config.has_review_policy(None, Some(Accept::Owner)));
    let global: Config = toml::from_str("[review]\naccept = \"owner\"").unwrap();
    assert!(global.has_review_policy(None, None));
}

#[test]
fn deprecated_max_diff_lines_alone_is_not_a_review_policy() {
    // Given layers that set only the removed, ignored max_diff_lines key.
    let global: Config = toml::from_str("[review]\nmax_diff_lines = 1500").unwrap();
    let project: Config = toml::from_str(
        "[projects.app]\npath = \".\"\n[projects.app.review]\nmax_diff_lines = 1500",
    )
    .unwrap();
    // Then they must not make review explicitly configured.
    assert!(!global.has_review_policy(None, None));
    assert!(!global.has_review_policy(Some("app"), None));
    assert!(!project.has_review_policy(Some("app"), None));
    assert!(global.review_policy(None, None).gate.is_empty());
    // And a real setting next to it still counts.
    let mixed: Config =
        toml::from_str("[review]\nmax_diff_lines = 1500\naccept = \"owner\"").unwrap();
    assert!(mixed.has_review_policy(None, None));
}

#[test]
fn map_keys_inherit_and_round_trip_without_replacing_other_keys() {
    let config: Config = toml::from_str(
        r#"
        [review]
        env = { SHARED = "global", GLOBAL = "kept" }
        size_limits = { rs = 700, py = 500 }
        [projects.app]
        path = "."
        [projects.app.review]
        env = { SHARED = "project", PROJECT = "added" }
        size_limits = { py = 300, ts = 400 }
        [projects.empty]
        path = "."
        [projects.empty.review]
        env = {}
        size_limits = {}
        "#,
    )
    .unwrap();
    let policy = config.review_policy(Some("app"), None);
    assert_eq!(
        policy.env,
        BTreeMap::from([
            ("GLOBAL".into(), "kept".into()),
            ("PROJECT".into(), "added".into()),
            ("SHARED".into(), "project".into()),
        ])
    );
    assert_eq!(
        policy.size_limits,
        BTreeMap::from([("rs".into(), 700), ("py".into(), 300), ("ts".into(), 400)])
    );
    let sources = config.review_policy_sources(Some("app"), None);
    assert_eq!(sources.env, PolicySource::Project);
    assert_eq!(sources.size_limits, PolicySource::Project);
    assert_eq!(
        config.review_policy(Some("empty"), None),
        config.review_policy(None, None)
    );
    assert_eq!(
        config
            .review_policy_sources(Some("empty"), None)
            .size_limits,
        PolicySource::Global
    );
    let roundtrip: Config = toml::from_str(&config.to_toml()).unwrap();
    assert_eq!(roundtrip, config);
    config.validate_review().unwrap();
}

#[test]
fn builtin_rust_limit_survives_empty_and_other_extension_overrides() {
    let config: Config = toml::from_str("[review]\nsize_limits = { py = 250 }\nenv = {}").unwrap();
    let policy = config.review_policy(None, None);
    assert_eq!(policy.size_limits["rs"], 800);
    assert_eq!(policy.size_limits["py"], 250);
    assert!(policy.env.is_empty());
    let serialized = serde_json::to_value(policy).unwrap();
    assert_eq!(serialized["size_limits"]["rs"], 800);
    assert!(serialized["env"].is_object());
}

#[test]
fn invalid_environment_identifiers_values_and_identity_overrides_are_rejected() {
    for key in [
        "",
        "1BAD",
        "A=B",
        "A-B",
        "with space",
        "é",
        "CORRAL_GATE_ATTEMPT",
        "CORRAL_CANDIDATE_HEAD",
        "CORRAL_INTEGRATION_BASE",
        "CORRAL_GATE_OWNER_TOKEN",
        "CORRAL_GATE_FUTURE",
        "CORRAL_SIZE_LIMITS",
    ] {
        let mut config: Config = toml::from_str("").unwrap();
        config.review.env = Some(BTreeMap::from([(key.into(), "value".into())]));
        assert!(config.validate_review().is_err(), "{key:?}");
    }
    let mut config: Config = toml::from_str("").unwrap();
    config.review.env = Some(BTreeMap::from([("VALID_1".into(), "with\0nul".into())]));
    assert!(config.validate_review().is_err());
    config.review.env = Some(BTreeMap::from([
        (
            "_PORTABLE_123".into(),
            "quotes ' and newline\nare valid".into(),
        ),
        ("CORRAL_HERDR_SESSION".into(), "corral-test".into()),
    ]));
    config.validate_review().unwrap();
}

#[test]
fn invalid_extensions_and_zero_limits_are_rejected_in_project_layers() {
    for extension in ["", ".rs", "*.rs", "src/rs", "a\\b", "a-b", "é"] {
        let mut config: Config = toml::from_str("[projects.app]\npath = \".\"").unwrap();
        config.projects.get_mut("app").unwrap().review.size_limits =
            Some(BTreeMap::from([(extension.into(), 100)]));
        assert!(config.validate_review().is_err(), "{extension:?}");
    }
    let config: Config = toml::from_str("[review]\nsize_limits = { rs = 0 }").unwrap();
    assert!(config.validate_review().is_err());
    assert!(toml::from_str::<Config>("[review]\nsize_limits = { rs = -1 }").is_err());
}
