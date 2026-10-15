use corral_core::*;

#[test]
fn per_tier_rules_override_only_selected_values() {
    // Given a workspace policy with different normal and transient behavior.
    let input = r#"
        [artifacts.retention]
        action = "compress"
        older_than = "14d"
        larger_than = "1MB"
        tiers = ["normal", "transient"]
        [artifacts.retention.per_tier.normal]
        action = "keep"
        [artifacts.retention.per_tier.transient]
        older_than = "2d"
        larger_than = "4KB"
    "#;
    let config: Config = toml::from_str(input).unwrap();

    // When the effective policies are selected by tier.
    let normal = config.artifacts.retention.for_tier(ArtifactTier::Normal);
    let transient = config.artifacts.retention.for_tier(ArtifactTier::Transient);

    // Then explicit values win while unrelated values inherit from the parent.
    assert_eq!(normal.action, RetentionAction::Keep);
    assert_eq!(normal.older_than, "14d");
    assert_eq!(transient.action, RetentionAction::Compress);
    assert_eq!(transient.older_than, "2d");
    assert_eq!(transient.larger_than.as_deref(), Some("4KB"));
    assert_eq!(normal.tiers, config.artifacts.retention.tiers);
    assert_eq!(toml::from_str::<Config>(&config.to_toml()).unwrap(), config);
}

#[test]
fn malformed_per_tier_thresholds_fail_config_validation() {
    // Given a selected tier with an invalid threshold rather than the valid parent default.
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default_for(temp.path());
    config.artifacts.retention.per_tier.insert(
        ArtifactTier::Transient,
        RetentionOverride {
            older_than: Some("yesterday".into()),
            larger_than: Some("infinite".into()),
            ..RetentionOverride::default()
        },
    );

    // When the complete configuration is validated.
    let errors = config.validate(temp.path()).unwrap_err();

    // Then both invalid boundary values are reported under the tier's config key.
    assert!(
        errors
            .0
            .iter()
            .any(|e| e.starts_with("artifacts.retention.per_tier.transient.older_than:"))
    );
    assert!(
        errors
            .0
            .iter()
            .any(|e| e.starts_with("artifacts.retention.per_tier.transient.larger_than:"))
    );
}

fn artifact(kind: ArtifactKind, path: &str, title: &str, bytes: u64) -> Artifact {
    Artifact {
        run: "R1".into(),
        kind,
        title: title.into(),
        path: path.into(),
        source: path.into(),
        sha256: "digest".into(),
        bytes,
        added: "2026-10-01T00:00:00Z".parse().unwrap(),
        tier_override: None,
        compressed_bytes: None,
        purged_at: None,
    }
}

#[test]
fn tiers_round_trip_when_serialized() {
    // Given all supported tier values.
    for tier in [
        ArtifactTier::Keep,
        ArtifactTier::Normal,
        ArtifactTier::Transient,
    ] {
        // When serialized as the public lowercase value.
        let json = serde_json::to_string(&tier).unwrap();
        // Then serde and the CLI parser agree.
        assert_eq!(json, format!("\"{}\"", tier.as_str()));
        assert_eq!(serde_json::from_str::<ArtifactTier>(&json).unwrap(), tier);
        assert_eq!(ArtifactTier::parse(tier.as_str()), Some(tier));
    }
    assert_eq!(ArtifactTier::parse(" KEEP "), Some(ArtifactTier::Keep));
    assert_eq!(ArtifactTier::parse("unknown"), None);
}

#[test]
fn tiers_are_inferred_when_no_override_exists() {
    // Given artifact kinds, names and byte sizes at the JSON boundary.
    let cases = [
        (
            ArtifactKind::Report,
            "result.json",
            "Result",
            2_000_000,
            ArtifactTier::Keep,
        ),
        (
            ArtifactKind::Summary,
            "summary.txt",
            "Summary",
            1,
            ArtifactTier::Keep,
        ),
        (
            ArtifactKind::Plan,
            "plan.txt",
            "Plan",
            1,
            ArtifactTier::Keep,
        ),
        (
            ArtifactKind::Screenshot,
            "shot.png",
            "Screenshot",
            1,
            ArtifactTier::Keep,
        ),
        (
            ArtifactKind::Evidence,
            "result.md",
            "Review",
            1,
            ArtifactTier::Keep,
        ),
        (
            ArtifactKind::Other,
            "acceptance.HTML",
            "Result",
            1,
            ArtifactTier::Keep,
        ),
        (
            ArtifactKind::Evidence,
            "evidence.md",
            "Evidence",
            1,
            ArtifactTier::Normal,
        ),
        (
            ArtifactKind::Evidence,
            "summary.md",
            "Result",
            1,
            ArtifactTier::Keep,
        ),
        (
            ArtifactKind::Other,
            "shot.PNG",
            "Result",
            1,
            ArtifactTier::Keep,
        ),
        (
            ArtifactKind::Evidence,
            "diagram.svg",
            "Diagram",
            1,
            ArtifactTier::Keep,
        ),
        (
            ArtifactKind::Other,
            "shot.bmp",
            "Screenshot",
            1,
            ArtifactTier::Keep,
        ),
        (
            ArtifactKind::Evidence,
            "reports/evidence.md",
            "Evidence",
            1,
            ArtifactTier::Normal,
        ),
        (
            ArtifactKind::Diff,
            "final.diff",
            "Final diff",
            2_000_000,
            ArtifactTier::Normal,
        ),
        (
            ArtifactKind::Log,
            "output.txt",
            "Output",
            1,
            ArtifactTier::Transient,
        ),
        (
            ArtifactKind::Evidence,
            "output.ANSI",
            "Output",
            1,
            ArtifactTier::Transient,
        ),
        (
            ArtifactKind::Other,
            "output.log",
            "Output",
            1,
            ArtifactTier::Transient,
        ),
        (
            ArtifactKind::Evidence,
            "data.json",
            "Data",
            999_999,
            ArtifactTier::Normal,
        ),
        (
            ArtifactKind::Evidence,
            "data.json",
            "Data",
            1_000_000,
            ArtifactTier::Transient,
        ),
    ];
    for (kind, path, title, bytes, expected) in cases {
        // When its effective tier is requested.
        let tier = artifact(kind, path, title, bytes).tier();
        // Then the classification follows the public inference contract.
        assert_eq!(tier, expected, "{path} / {title}");
    }
}

#[test]
fn override_wins_when_kind_would_keep() {
    // Given a report explicitly classified as transient.
    let mut value = artifact(ArtifactKind::Report, "report.md", "Report", 1);
    value.tier_override = Some(ArtifactTier::Transient);
    // When its effective tier is requested.
    let tier = value.tier();
    // Then the explicit classification wins.
    assert_eq!(tier, ArtifactTier::Transient);
}

#[test]
fn metadata_defaults_when_legacy_artifact_is_loaded() {
    // Given a legacy artifact payload without optional storage metadata.
    let json = r#"{"run":"R1","kind":"evidence","title":"Data","path":"data.json",
        "source":"/data.json","sha256":"digest","bytes":10,"added":"2026-10-01T00:00:00Z"}"#;
    // When decoded and serialized again.
    let value: Artifact = serde_json::from_str(json).unwrap();
    let encoded = serde_json::to_value(&value).unwrap();
    // Then absent fields remain absent rather than changing legacy records.
    assert_eq!(value.tier_override, None);
    assert_eq!(value.compressed_bytes, None);
    assert_eq!(value.purged_at, None);
    for key in ["tier_override", "compressed_bytes", "purged_at"] {
        assert!(encoded.get(key).is_none());
    }
}

#[test]
fn metadata_round_trips_when_present() {
    // Given a compressed and subsequently purged artifact record.
    let mut value = artifact(ArtifactKind::Log, "output.log", "Log", 1_000);
    value.tier_override = Some(ArtifactTier::Normal);
    value.compressed_bytes = Some(250);
    value.purged_at = Some("2026-10-14T00:00:00Z".parse().unwrap());
    // When the YAML record is round-tripped.
    let encoded = serde_yaml_ng::to_string(&value).unwrap();
    let decoded: Artifact = serde_yaml_ng::from_str(&encoded).unwrap();
    // Then storage metadata survives unchanged.
    assert_eq!(decoded, value);
}

#[test]
fn retention_defaults_when_old_config_is_loaded() {
    // Given the pre-retention artifact configuration.
    let old = "[artifacts]\nglobs = []\nmax_bytes = 12345\n[projects.app]\npath = \".\"\n";
    // When loaded through the config parser.
    let config: Config = toml::from_str(old).unwrap();
    // Then new defaults apply without altering the hard admission limit.
    assert_eq!(config.artifacts.max_bytes, 12345);
    assert_eq!(config.artifacts.max_total, "500MB");
    assert_eq!(config.artifacts.max_file, "20MB");
    assert_eq!(config.artifacts.retention, ArtifactRetention::default());
    assert_eq!(config.artifacts.retention.action, RetentionAction::Compress);
    assert_eq!(config.artifacts.retention.older_than, "14d");
    assert_eq!(
        config.artifacts.retention.larger_than.as_deref(),
        Some("1MB")
    );
    assert_eq!(
        config.artifacts.retention.tiers,
        vec![ArtifactTier::Transient, ArtifactTier::Normal]
    );
    assert_eq!(config.projects["app"].artifact_retention, None);
}

#[test]
fn retention_round_trips_when_project_policy_is_nested() {
    // Given separate global and project policies in their supported TOML tables.
    let input = r#"
        [artifacts.retention]
        action = "compress"
        older_than = "2w"
        tiers = ["transient"]
        [projects.app]
        path = "."
        [projects.app.artifacts.retention]
        action = "keep"
        older_than = "3d"
        larger_than = "2MiB"
        tiers = ["normal", "keep"]
    "#;
    // When parsed and round-tripped.
    let config: Config = toml::from_str(input).unwrap();
    let encoded = config.to_toml();
    let decoded: Config = toml::from_str(&encoded).unwrap();
    // Then the ergonomic TOML key preserves the direct Rust API.
    assert_eq!(config, decoded);
    assert!(encoded.contains("[projects.app.artifacts.retention]"));
    let policy = decoded.projects["app"].artifact_retention.as_ref().unwrap();
    assert_eq!(policy.action, RetentionAction::Keep);
    assert_eq!(policy.older_than, "3d");
    assert_eq!(policy.larger_than.as_deref(), Some("2MiB"));
}

#[test]
fn byte_sizes_parse_when_supported_units_are_used() {
    // Given decimal and binary byte-size units, including the u64 boundary.
    for (input, expected) in [
        ("1B", 1),
        ("2KB", 2_000),
        ("3MB", 3_000_000),
        ("4GB", 4_000_000_000),
        ("2KiB", 2_048),
        ("3MiB", 3_145_728),
        ("4GiB", 4_294_967_296),
        (" 5 MB ", 5_000_000),
        ("18446744073709551615B", u64::MAX),
    ] {
        // When parsed.
        let bytes = parse_bytes(input).unwrap();
        // Then the conversion is exact.
        assert_eq!(bytes, expected, "{input}");
    }
}

#[test]
fn byte_sizes_fail_when_invalid_or_overflowing() {
    // Given malformed counts and values outside the supported domain.
    for input in [
        "",
        "1",
        "0B",
        "-1MB",
        "1.5MB",
        "1TB",
        "1mb",
        "MB",
        "18446744073709551616B",
        "18446744073709551615KB",
    ] {
        // When parsed.
        let result = parse_bytes(input);
        // Then no overflow, zero or unsupported size is admitted.
        assert!(result.is_err(), "{input}");
    }
}

#[test]
fn validation_reports_global_and_project_retention_errors() {
    // Given invalid global/project policy and soft thresholds in an otherwise valid config.
    let root = tempfile::tempdir().unwrap();
    let mut config = Config::default_for(root.path());
    config.artifacts.retention.older_than = "0d".into();
    config.artifacts.retention.larger_than = Some("0B".into());
    config.artifacts.max_total = "bad".into();
    config.artifacts.max_file = "18446744073709551615GB".into();
    let project: Project = toml::from_str(
        "path = \".\"\n[artifacts.retention]\nolder_than = \"-1d\"\nlarger_than = \"wat\"\n",
    )
    .unwrap();
    config.projects.insert("app".into(), project);
    // When validation aggregates errors.
    let errors = config.validate(root.path()).unwrap_err().0;
    // Then every invalid setting is identified by its actual config key.
    for key in [
        "artifacts.retention.older_than",
        "artifacts.retention.larger_than",
        "artifacts.max_total",
        "artifacts.max_file",
        "projects.app.artifacts.retention.older_than",
        "projects.app.artifacts.retention.larger_than",
    ] {
        assert!(
            errors.iter().any(|error| error.starts_with(key)),
            "{key}: {errors:?}"
        );
    }
}

#[test]
fn default_retention_validates_without_repository_or_external_services() {
    // Given the default policy in an isolated empty workspace.
    let root = tempfile::tempdir().unwrap();
    let config = Config::default_for(root.path());
    // When validated.
    let result = config.validate(root.path());
    // Then the shipped defaults are valid without git or herdr.
    assert!(result.is_ok(), "{result:?}");
}
