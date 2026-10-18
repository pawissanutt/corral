use super::*;

#[test]
fn staged_repair_policy_is_explicit_and_legacy_limits_stay_unchanged() {
    // Given an existing manual configuration.
    let (dir, mut config) = config();
    assert!(config.routing.repair.is_none());
    // When an explicit staged policy is serialized and read back.
    config.routing.repair = Some(RepairPolicy::approved_starter_policy());
    let decoded: Config = toml::from_str(&config.to_toml()).unwrap();
    // Then the new fields persist without reinterpreting existing limits.
    assert_eq!(decoded.routing.repair, config.routing.repair);
    assert_eq!(decoded.routing.escalate_after, 1);
    assert_eq!(decoded.review_policy(None, None).max_rounds, 2);
    assert!(decoded.validate(dir.path()).is_ok());
    let mut invalid = decoded;
    invalid.routing.repair = Some(RepairPolicy::V1 {
        current_profile_limit: 1,
        stronger_profile_limit: 1,
        overall_limit: 3,
        pm_diagnosis_allowance: 1,
    });
    assert!(invalid.validate(dir.path()).is_err());
}

#[test]
fn metered_candidates_cannot_exceed_currency_or_spending_bounds() {
    // Given approved metered candidates bounded in one currency.
    let (_dir, config) = config();
    for case in ["within", "amount", "currency", "provider"] {
        let mut profile = with_fallback();
        let metadata = profile.metadata.as_mut().unwrap();
        metadata.fallback.as_mut().unwrap().spending = Some(SpendingBound {
            currency: "USD".into(),
            maximum_micros: 100,
        });
        for candidate in &mut metadata.candidates {
            candidate.cost = CandidateCost::Metered {
                currency: "USD".into(),
                maximum_micros: 50,
            };
        }
        match case {
            "within" => {}
            "amount" => {
                metadata.candidates[1].cost = CandidateCost::Metered {
                    currency: "USD".into(),
                    maximum_micros: 101,
                }
            }
            "currency" => {
                metadata.candidates[1].cost = CandidateCost::Metered {
                    currency: "EUR".into(),
                    maximum_micros: 1,
                }
            }
            "provider" => metadata.candidates[1].provider = "openai".into(),
            _ => unreachable!(),
        }
        // When checking independently specified candidate costs and provider identities.
        let valid = profile
            .metadata
            .as_ref()
            .unwrap()
            .validate(&config, &profile)
            .is_ok();
        // Then only matching currency and in-bound spending is accepted.
        assert_eq!(valid, case == "within", "{case}");
    }
}

#[test]
fn legacy_manual_profiles_resolve_without_new_metadata() {
    // Given a manually configured pre-catalog preset.
    let (dir, mut config) = config();
    let legacy: Profile =
        toml::from_str("harness='omo'\nmodel='openai/legacy'\neffort='low'").unwrap();
    assert!(legacy.metadata.is_none());
    config.profiles.insert("manual".into(), legacy);
    // When its configuration is serialized and resolved.
    let decoded: Config = toml::from_str(&config.to_toml()).unwrap();
    let resolved = decoded
        .resolve_profile(
            &ProfileChoice {
                profile: Some("manual".into()),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
    // Then execution and existing limits are unchanged.
    assert_eq!(
        resolved.model_args,
        ["--model", "openai/legacy", "--thinking", "low"]
    );
    assert_eq!(decoded.routing.escalate_after, 1);
    assert_eq!(decoded.review_policy(None, None).max_rounds, 2);
    assert!(decoded.validate(dir.path()).is_ok());
}

#[test]
fn renamed_display_preserves_custom_key_and_execution() {
    // Given a custom profile with separate guidance and worker instructions.
    let (_dir, mut config) = config();
    let before = config
        .resolve_profile(
            &ProfileChoice {
                profile: Some("custom.database".into()),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
    let revision = config.profile_catalog_revision().unwrap();
    // When the display label changes and TOML round-trips.
    config
        .profiles
        .get_mut("custom.database")
        .unwrap()
        .metadata
        .as_mut()
        .unwrap()
        .display_name = "Storage".into();
    let decoded: Config = toml::from_str(&config.to_toml()).unwrap();
    // Then the same stable key resolves the same execution, but invalidates catalog receipts.
    assert_eq!(
        decoded
            .resolve_profile(
                &ProfileChoice {
                    profile: Some("custom.database".into()),
                    ..Default::default()
                },
                None,
                None
            )
            .unwrap(),
        before
    );
    assert_ne!(decoded.profile_catalog_revision().unwrap(), revision);
    assert_eq!(
        decoded.profiles["custom.database"].metadata,
        config.profiles["custom.database"].metadata
    );
}

#[test]
fn eligibility_is_enforced_for_role_auto_enabled_and_project() {
    // Given custom machine-consumed eligibility rather than prose.
    let (_dir, mut config) = config();
    let m = config
        .profiles
        .get_mut("custom.database")
        .unwrap()
        .metadata
        .as_mut()
        .unwrap();
    m.eligibility.roles = BTreeSet::from([ProfileRole::ImplementationReview]);
    m.eligibility.automatic = false;
    // When checking manual review and excluded role/Auto requests.
    let check = |role, automatic| {
        config.check_profile_eligibility("custom.database", role, None, automatic)
    };
    // Then manual review is allowed, but neither instructions nor a key name bypass constraints.
    assert!(check(ProfileRole::ImplementationReview, false).is_ok());
    assert!(check(ProfileRole::Implementation, false).is_err());
    assert!(check(ProfileRole::ImplementationReview, true).is_err());
    assert!(
        config
            .check_profile_eligibility("missing", ProfileRole::Implementation, None, false)
            .is_err()
    );
    assert!(
        config
            .check_profile_eligibility(
                "custom.database",
                ProfileRole::ImplementationReview,
                Some("unknown-project"),
                false
            )
            .is_err()
    );
}

#[test]
fn invalid_catalog_scope_and_empty_role_set_fail_config_validation() {
    // Given malformed custom eligibility.
    for malformed in ["project", "roles", "disabled"] {
        let (dir, mut config) = config();
        let m = config
            .profiles
            .get_mut("custom.database")
            .unwrap()
            .metadata
            .as_mut()
            .unwrap();
        match malformed {
            "project" => {
                m.eligibility.projects.insert("missing".into());
            }
            "roles" => m.eligibility.roles.clear(),
            "disabled" => m.eligibility.enabled = false,
            _ => unreachable!(),
        }
        // When validating config and selection eligibility.
        let result = config.validate(dir.path());
        // Then malformed scopes/roles fail, and disabled profiles cannot be selected.
        if malformed == "disabled" {
            assert!(
                config
                    .check_profile_eligibility(
                        "custom.database",
                        ProfileRole::Implementation,
                        None,
                        false
                    )
                    .is_err()
            );
        } else {
            assert!(result.is_err());
        }
    }
}

fn with_fallback() -> Profile {
    let mut profile = custom_profile();
    let m = profile.metadata.as_mut().unwrap();
    let mut fallback = candidate("fallback");
    fallback.harness = "claude".into();
    fallback.provider = "anthropic".into();
    fallback.model = "anthropic/exact-fixture-v2".into();
    fallback.effort = Some("max".into());
    m.candidates.push(fallback);
    m.fallback = Some(AvailabilityFallback {
        automatic: true,
        approved_providers: BTreeSet::from(["openai".into(), "anthropic".into()]),
        max_switches: 1,
        allow_unknown_subscription_cost: true,
        spending: None,
        approval: "proposal-1".into(),
    });
    profile
}

#[test]
fn ordered_candidates_validate_their_own_harness_settings() {
    // Given independently configured preferred and fallback candidates.
    let (_dir, config) = config();
    let mut profile = with_fallback();
    assert!(
        profile
            .metadata
            .as_ref()
            .unwrap()
            .validate(&config, &profile)
            .is_ok()
    );
    // When an effort valid for OMO but invalid for Claude is copied to the fallback.
    profile.metadata.as_mut().unwrap().candidates[1].effort = Some("off".into());
    // Then validation rejects it without changing difficulty escalation.
    assert!(
        profile
            .metadata
            .as_ref()
            .unwrap()
            .validate(&config, &profile)
            .is_err()
    );
    assert!(profile.escalate_to.is_none());
}

#[test]
fn fallback_rejects_unapproved_provider_unknown_cost_and_unbounded_switches() {
    // Given an approved finite list and independent failure cases.
    let (_dir, config) = config();
    for case in [
        "provider",
        "cost",
        "switches",
        "duplicate",
        "preferred",
        "no-policy",
    ] {
        let mut profile = with_fallback();
        let m = profile.metadata.as_mut().unwrap();
        match case {
            "provider" => {
                m.fallback
                    .as_mut()
                    .unwrap()
                    .approved_providers
                    .remove("anthropic");
            }
            "cost" => {
                m.fallback.as_mut().unwrap().spending = Some(SpendingBound {
                    currency: "USD".into(),
                    maximum_micros: 1,
                });
            }
            "switches" => m.fallback.as_mut().unwrap().max_switches = 2,
            "duplicate" => m.candidates[1].id = "preferred".into(),
            "preferred" => m.candidates[0].model = "openai/different".into(),
            "no-policy" => m.fallback = None,
            _ => unreachable!(),
        }
        // When validating the candidate policy.
        let result = profile
            .metadata
            .as_ref()
            .unwrap()
            .validate(&config, &profile);
        // Then no alternate can exceed an approved bound.
        assert!(result.is_err(), "{case}");
    }
}

#[test]
fn scoped_profiles_accept_only_named_configured_projects() {
    // Given a profile scoped to an existing configured project.
    let (dir, mut config) = config();
    config
        .projects
        .insert("app".into(), toml::from_str("path='.'").unwrap());
    config
        .profiles
        .get_mut("custom.database")
        .unwrap()
        .metadata
        .as_mut()
        .unwrap()
        .eligibility
        .projects
        .insert("app".into());
    // When checking the catalog and project-specific selections.
    assert!(config.validate(dir.path()).is_ok());
    // Then a projectless request is excluded but the named project is eligible.
    assert!(
        config
            .check_profile_eligibility("custom.database", ProfileRole::Implementation, None, true)
            .is_err()
    );
    assert!(
        config
            .check_profile_eligibility(
                "custom.database",
                ProfileRole::Implementation,
                Some("app"),
                true
            )
            .is_ok()
    );
}
