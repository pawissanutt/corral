use corral_core::{Config, ProfileChoice, Reviewer, Routing, infer_vendor};

fn fixture() -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    let profiles: Config = toml::from_str(
        r#"
[profiles.claude]
harness = "omo"
model = "anthropic-subscription/claude"
escalate_to = "codex"
[profiles.codex]
harness = "omo"
model = "openai-codex:gpt"
[profiles.other]
harness = "omo"
model = "google/gemini"
[profiles.unknown]
harness = "omo"
model = "unqualified"
[routing.reviewer_by_vendor]
anthropic = "claude"
openai = "codex"
"#,
    )
    .unwrap();
    config.profiles.extend(profiles.profiles);
    config.routing = profiles.routing;
    (dir, config)
}

#[test]
fn vendor_inference_normalizes_provider_prefixes() {
    // Given provider-qualified models, known bare families and unknown names.
    for (model, expected) in [
        ("anthropic/claude", Some("anthropic")),
        ("anthropic-subscription/claude", Some("anthropic")),
        ("anthropic-subscription:claude", Some("anthropic")),
        ("chatgpt-subscription/gpt", Some("openai")),
        ("chatgpt-subscription:gpt", Some("openai")),
        ("openai-codex/gpt", Some("openai")),
        ("openai-codex:gpt", Some("openai")),
        ("openai/gpt", Some("openai")),
        ("custom/model/version", Some("custom")),
        ("custom:model/version", Some("custom")),
        ("claude-sonnet-4-5", Some("anthropic")),
        ("gpt-5", Some("openai")),
        ("o3", Some("openai")),
        ("o4-mini", Some("openai")),
        ("o", None),
        ("other-model", None),
        ("claude", None),
        ("gpt", None),
        ("model", None),
        ("", None),
        ("/model", None),
        (":model", None),
        ("provider/", None),
        ("provider:", None),
    ] {
        // When inferring a vendor, then prefixes take precedence over bare families.
        assert_eq!(infer_vendor(model).as_deref(), expected, "{model}");
    }
}

fn historical_run() -> corral_core::Run {
    serde_json::from_value(serde_json::json!({
        "id": "R1", "agent": "claude", "worktree": "/unused",
        "branch": "corral/T-001", "base": "main", "base_sha": "abc",
        "started": "2026-10-15T00:00:00Z"
    }))
    .unwrap()
}

#[test]
fn run_vendor_uses_snapshot_then_current_profile_in_order() {
    // Given conflicting vendor sources, or absent and unrecognizable historical metadata.
    for (vendor, model, profile_vendor, profile_model, expected) in [
        (
            Some("custom"),
            Some("gpt-5"),
            Some("openai"),
            Some("claude-sonnet-4-5"),
            "custom",
        ),
        (
            None,
            Some("claude-sonnet-4-5"),
            Some("openai"),
            Some("gpt-5"),
            "anthropic",
        ),
        (None, None, Some("custom"), Some("gpt-5"), "custom"),
        (
            None,
            Some("unknown"),
            Some("custom"),
            Some("gpt-5"),
            "custom",
        ),
        (None, None, None, Some("gpt-5"), "openai"),
        (None, Some("unknown"), None, Some("o3"), "openai"),
    ] {
        let (_dir, mut config) = fixture();
        let profile = config.profiles.get_mut("claude").unwrap();
        profile.vendor = profile_vendor.map(str::to_string);
        profile.model = profile_model.map(str::to_string);
        let mut run = historical_run();
        run.vendor = vendor.map(str::to_string);
        run.model = model.map(str::to_string);
        // When resolving the implementer vendor.
        let resolved = config.run_vendor(&run).unwrap();
        // Then the first usable source wins.
        assert_eq!(resolved, expected);
    }
}

#[test]
fn run_vendor_is_typed_when_profile_is_missing_or_unknown() {
    for profile in ["unknown", "missing", "omo"] {
        // Given an unknown model, an absent profile, or a legacy agent with no model.
        let (_dir, config) = fixture();
        let mut run = historical_run();
        run.agent = profile.into();
        // When resolution fails and the caller adds context.
        let err = anyhow::Error::new(config.run_vendor(&run).unwrap_err()).context("review");
        // Then detection needs no string matching and identifies the affected run.
        let unresolved = err
            .downcast_ref::<corral_core::UnresolvedRunVendor>()
            .unwrap();
        assert_eq!(unresolved.run, "R1");
        assert_eq!(unresolved.profile, profile);
    }
}

#[test]
fn run_vendor_prefers_explicit_profile_over_legacy_agent() {
    // Given imported metadata with both a harness/agent and a named profile.
    let (_dir, config) = fixture();
    let mut run = historical_run();
    run.agent = "omo".into();
    run.profile = Some("codex".into());
    // When the run round-trips through storage and resolves its vendor.
    let imported = serde_json::from_value(serde_json::to_value(&run).unwrap()).unwrap();
    // Then the explicit profile identifies the vendor.
    assert_eq!(config.run_vendor(&imported).unwrap(), "openai");
}

#[test]
fn resolved_vendor_uses_effective_model_unless_explicitly_overridden() {
    // Given inferred and explicit profile vendors, including model overrides.
    for (vendor, model, expected) in [
        (None, None, Some("anthropic")),
        (None, Some("openai/gpt"), Some("openai")),
        (None, Some("unqualified"), None),
        (Some("custom"), None, Some("custom")),
        (Some("custom"), Some("openai/gpt"), Some("custom")),
    ] {
        let (_dir, mut config) = fixture();
        config.profiles.get_mut("claude").unwrap().vendor = vendor.map(str::to_string);
        // When resolving the profile.
        let resolved = config
            .resolve_profile(
                &ProfileChoice {
                    profile: Some("claude".into()),
                    model: model.map(str::to_string),
                    ..Default::default()
                },
                None,
                None,
            )
            .unwrap();
        // Then explicit vendors win; otherwise inference sees the overridden model.
        assert_eq!(resolved.vendor.as_deref(), expected);
    }
}

#[test]
fn routing_defaults_and_disabled_escalation_round_trip() {
    // Given omitted, default, disabled and customized thresholds.
    for threshold in [None, Some(1), Some(0), Some(3)] {
        let text = threshold
            .map(|value| format!("[routing]\nescalate_after = {value}\n"))
            .unwrap_or_default();
        let config: Config = toml::from_str(&text).unwrap();
        // When serializing and parsing configuration.
        let serialized = config.to_toml();
        let value: toml::Value = toml::from_str(&serialized).unwrap();
        let decoded: Config = toml::from_str(&serialized).unwrap();
        // Then nondefaults survive while defaults omit the routing table.
        assert_eq!(config.routing.escalate_after, threshold.unwrap_or(1));
        assert_eq!(decoded.routing, config.routing);
        assert_eq!(value.get("routing").is_none(), threshold.unwrap_or(1) == 1);
        assert_eq!(config.routing.is_empty(), threshold.unwrap_or(1) == 1);
    }
    assert_eq!(Routing::default().escalate_after, 1);
}

#[test]
fn reviewer_mapping_and_profile_fields_round_trip() {
    // Given vendor mappings and a profile with an explicit vendor and escalation target.
    let (_dir, mut config) = fixture();
    config.profiles.get_mut("claude").unwrap().vendor = Some("anthropic".into());
    // When serializing and parsing.
    let decoded: Config = toml::from_str(&config.to_toml()).unwrap();
    // Then all configuration fields survive even without routing rules.
    assert!(!config.routing.is_empty());
    assert_eq!(decoded, config);
    assert_eq!(
        decoded.profiles["claude"].escalate_to.as_deref(),
        Some("codex")
    );
}

#[test]
fn cross_vendor_reviewer_selects_opposite_vendor_in_both_directions() {
    // Given mappings keyed by the reviewer's vendor.
    let (_dir, config) = fixture();
    for (implementer, expected) in [("anthropic", "codex"), ("openai", "claude")] {
        // When selecting a reviewer, then the opposite vendor's profile wins.
        assert_eq!(config.cross_vendor_reviewer(implementer).unwrap(), expected);
    }
}

#[test]
fn other_vendor_requires_exactly_one_different_mapping() {
    for (vendors, expected) in [
        (
            vec![("google", "other"), ("openai", "codex")],
            Some("codex"),
        ),
        (vec![("google", "other")], None),
        (vec![], None),
        (vec![("anthropic", "claude"), ("openai", "codex")], None),
    ] {
        // Given zero, one or multiple different reviewer vendors.
        let (_dir, mut config) = fixture();
        config.routing.reviewer_by_vendor = vendors
            .into_iter()
            .map(|(vendor, profile)| (vendor.into(), profile.into()))
            .collect();
        // When selecting for a third-party implementer.
        let selected = config.cross_vendor_reviewer("google");
        // Then only a sole different mapping succeeds.
        match expected {
            Some(profile) => assert_eq!(selected.unwrap(), profile),
            None => assert!(selected.is_err()),
        }
    }
}

#[test]
fn cross_vendor_reviewer_rejects_missing_unknown_and_mismatched_vendors() {
    for (implementer, mapped_profile) in [
        ("anthropic", None),
        ("anthropic", Some("missing")),
        ("anthropic", Some("unknown")),
        ("anthropic", Some("claude")),
        ("", Some("codex")),
        (" ", Some("codex")),
    ] {
        // Given a missing mapping, unresolved profile, unknown vendor or same-vendor profile.
        let (_dir, mut config) = fixture();
        match mapped_profile {
            Some(profile) => {
                config
                    .routing
                    .reviewer_by_vendor
                    .insert("openai".into(), profile.into());
            }
            None => {
                config.routing.reviewer_by_vendor.remove("openai");
            }
        }
        // When selecting a reviewer, then misconfiguration fails rather than falling back.
        assert!(config.cross_vendor_reviewer(implementer).is_err());
    }
}

#[test]
fn reviewer_mapping_checks_resolved_explicit_vendor() {
    // Given a model whose inferred vendor disagrees with its explicit override.
    let (_dir, mut config) = fixture();
    config.profiles.get_mut("codex").unwrap().vendor = Some("anthropic".into());
    // When selecting its mapping, then the resolved vendor rejects the mismatch.
    assert!(config.cross_vendor_reviewer("anthropic").is_err());
}

#[test]
fn validation_accepts_finite_escalation_and_cross_vendor_policy() {
    // Given a finite chain and cross-vendor policy at both review layers.
    let (dir, mut config) = fixture();
    config.review.reviewer = Some("cross-vendor".into());
    config.projects.insert(
        "app".into(),
        toml::from_str("path = '.'\n[review]\nreviewer = 'cross-vendor'\n").unwrap(),
    );
    // When validating, then the chain and review keyword are accepted.
    config.validate(dir.path()).unwrap();
    config.validate_review().unwrap();
    assert_eq!(Reviewer::parse("cross-vendor"), Reviewer::CrossVendor);
    assert_eq!(
        config.review_policy(Some("app"), None).reviewer,
        Reviewer::CrossVendor
    );
}

#[test]
fn validation_rejects_unknown_escalation_targets_and_cycles() {
    for (first, second) in [
        ("missing", None),
        ("claude", None),
        ("codex", Some("claude")),
        ("codex", Some("other")),
    ] {
        // Given an unknown target, self-loop, two-node cycle or longer cycle.
        let (dir, mut config) = fixture();
        config.profiles.get_mut("claude").unwrap().escalate_to = Some(first.into());
        config.profiles.get_mut("codex").unwrap().escalate_to = second.map(str::to_string);
        config.profiles.get_mut("other").unwrap().escalate_to = Some("claude".into());
        // When validating, then no unbounded escalation chain is accepted.
        assert!(config.validate(dir.path()).is_err());
    }
}

#[test]
fn validation_rejects_bad_reviewer_map_references_and_vendors() {
    for (vendor, profile) in [
        ("openai", "missing"),
        ("openai", "unknown"),
        ("openai", "claude"),
        ("", "codex"),
    ] {
        // Given an invalid reviewer mapping.
        let (dir, mut config) = fixture();
        config.routing.reviewer_by_vendor.clear();
        config
            .routing
            .reviewer_by_vendor
            .insert(vendor.into(), profile.into());
        // When validating, then both entry points reject it.
        assert!(config.validate(dir.path()).is_err());
        assert!(config.validate_review().is_err());
    }
}
