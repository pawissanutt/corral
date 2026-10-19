use super::*;

fn starter_input(mut input: SetupProposalInput, provider: &str) -> SetupProposalInput {
    let mut profile = input.proposal.profiles["database"].clone();
    let model = format!("{provider}/fixture-v1");
    profile.model = Some(model.clone());
    profile.vendor = Some(provider.into());
    let metadata = profile.metadata.as_mut().unwrap();
    metadata.eligibility.roles = BTreeSet::from([
        ProfileRole::Implementation,
        ProfileRole::Planning,
        ProfileRole::Pm,
        ProfileRole::PlanReview,
        ProfileRole::ImplementationReview,
    ]);
    metadata.candidates[0].provider = provider.into();
    metadata.candidates[0].model = model.clone();
    input.discovery.harnesses[0].models = vec![model.clone()];
    input.discovery.candidates[0].provider = provider.into();
    input.discovery.candidates[0].model = model;
    let recommendation = input.proposal.recommendations["database"].clone();
    let access = input.proposal.access[0].clone();
    input.proposal.profiles.clear();
    input.proposal.recommendations.clear();
    input.proposal.access.clear();
    for key in STARTER_PROFILE_KEYS {
        let mut profile = profile.clone();
        let metadata = profile.metadata.as_mut().unwrap();
        metadata.display_name = key.into();
        metadata.revision = format!("{key}-r1");
        input.proposal.profiles.insert(key.into(), profile);
        input
            .proposal
            .recommendations
            .insert(key.into(), recommendation.clone());
        input.proposal.access.push(CandidateAccess {
            profile: key.into(),
            ..access.clone()
        });
    }
    input.proposal.roles = BTreeMap::from([
        (ProfileRole::Implementation, "general".into()),
        (ProfileRole::Planning, "planning".into()),
        (ProfileRole::Pm, "deep".into()),
        (ProfileRole::PlanReview, "design-review".into()),
        (
            ProfileRole::ImplementationReview,
            "implementation-review".into(),
        ),
    ]);
    input.proposal.preferences.allowed_providers = vec![provider.into()];
    input.proposal.preferences.provider_preference = vec![provider.into()];
    input
}

fn project(review: ReviewOverride) -> Project {
    Project {
        path: ".".into(),
        default_profile: None,
        base: None,
        review,
        dispatch: Default::default(),
        artifact_retention: None,
        post_land: vec![],
    }
}

fn metered_fallback_input(mut input: SetupProposalInput) -> SetupProposalInput {
    let bound = SpendingBound {
        currency: "USD".into(),
        maximum_micros: 900,
    };
    input.proposal.preferences.allowed_providers =
        vec!["openai".into(), "anthropic".into(), "google".into()];
    input.proposal.preferences.provider_preference =
        input.proposal.preferences.allowed_providers.clone();
    input.proposal.preferences.spending = Some(bound.clone());
    let metadata = input
        .proposal
        .profiles
        .get_mut("database")
        .unwrap()
        .metadata
        .as_mut()
        .unwrap();
    metadata.candidates[0].cost = CandidateCost::Metered {
        currency: "USD".into(),
        maximum_micros: 800,
    };
    for (id, provider, maximum_micros) in [("second", "anthropic", 500), ("third", "google", 200)] {
        let mut candidate = metadata.candidates[0].clone();
        candidate.id = id.into();
        candidate.provider = provider.into();
        candidate.model = format!("{provider}/fixture-fallback-v1");
        candidate.connection = format!("owner-{provider}");
        candidate.cost = CandidateCost::Metered {
            currency: "USD".into(),
            maximum_micros,
        };
        let mut observation = input.discovery.candidates[0].clone();
        observation.provider = candidate.provider.clone();
        observation.model = candidate.model.clone();
        observation.connection = candidate.connection.clone();
        input.discovery.harnesses[0]
            .models
            .push(candidate.model.clone());
        input.discovery.candidates.push(observation);
        input.proposal.access.push(CandidateAccess {
            candidate: id.into(),
            ..input.proposal.access[0].clone()
        });
        metadata.candidates.push(candidate);
    }
    metadata.fallback = Some(AvailabilityFallback {
        automatic: true,
        approved_providers: BTreeSet::from(["openai".into(), "anthropic".into(), "google".into()]),
        max_switches: 2,
        allow_unknown_subscription_cost: false,
        spending: Some(bound),
        approval: input.proposal.id.clone(),
    });
    input
}

#[test]
fn sourced_nine_profile_setup_applies_every_role_and_exact_evidence() {
    // Given the complete starter catalog with independent local support facts.
    let (dir, store, input) = fixture();
    let mut input = starter_input(input, "openai");
    input.proposal.procedure_version = MODEL_SELECTION_VERSION.into();
    let comparison = ModelEvidence {
        version: 1,
        id: "comparison".into(),
        source: ModelEvidenceSource::Web {
            url: "https://comparison.example.test/models".into(),
        },
        retrieved_at: now(),
        published_on: Some("2026-09-02".parse().unwrap()),
        measured_on: Some("2026-09-01".parse().unwrap()),
        claim: "Fixture task-specific comparison".into(),
        finding: "Fixture result at the stated setting".into(),
        limitations: vec!["Old measurement; no coding-harness evaluation".into()],
    };
    input
        .proposal
        .evidence
        .insert(comparison.id.clone(), comparison.clone());
    for recommendation in input.proposal.recommendations.values_mut() {
        recommendation.evidence.push(comparison.id.clone());
        recommendation
            .uncertainty
            .extend(comparison.limitations.clone());
    }
    let authority = store.config.automation.clone();
    let receipt = approval(&input);
    let record = proposed(&store, &input);
    setup_approve(&store, &receipt, record.revision).unwrap();

    // When the owner-approved catalog is applied and reopened.
    let applied = setup_apply(&store).unwrap();
    let reopened = Store::open_unobserved(dir.path()).unwrap();
    let catalog = setup_catalog(&reopened).unwrap();

    // Then all nine exact profiles and five role bindings survive the transaction.
    assert_eq!(catalog.starter_keys, STARTER_PROFILE_KEYS);
    assert_eq!(input.proposal.profiles.len(), 9);
    assert_eq!(catalog.roles, input.proposal.roles);
    for (key, profile) in &input.proposal.profiles {
        assert_eq!(&catalog.profiles[key], profile);
    }
    assert_eq!(
        catalog.default_profile.as_ref(),
        input.proposal.roles.get(&ProfileRole::Implementation)
    );
    assert_eq!(catalog.pm_profile, input.proposal.roles[&ProfileRole::Pm]);
    assert_eq!(
        catalog.reviewer.as_ref(),
        input.proposal.roles.get(&ProfileRole::ImplementationReview)
    );
    assert_eq!(applied.proposal.as_ref(), Some(&input.proposal));
    assert_eq!(applied.approval.as_ref(), Some(&receipt));
    assert_eq!(applied.state, ProfileSetupState::Applied);
    assert_eq!(
        applied.validation,
        SetupValidationFacets {
            configuration_valid: true,
            access_checked: false,
            execution_tested: false,
        }
    );
    assert_eq!(reopened.config.automation, authority);
    assert!(reopened.list().unwrap().is_empty());
}

#[test]
fn provider_only_catalogs_use_the_single_available_model_without_fallback() {
    // Given OpenAI-only, Anthropic-only, and another-provider-only inventories.
    for provider in ["openai", "anthropic", "google"] {
        let (dir, store, input) = fixture();
        let input = starter_input(input, provider);
        let expected_candidate = input.proposal.profiles["general"]
            .metadata
            .as_ref()
            .unwrap()
            .candidates[0]
            .clone();
        approved(&store, &input);

        // When each one-model inventory is applied without adding alternatives.
        setup_apply(&store).unwrap();
        let reopened = Store::open_unobserved(dir.path()).unwrap();
        let catalog = setup_catalog(&reopened).unwrap();

        // Then every starter uses only that supplied model and provider.
        assert_eq!(input.discovery.candidates.len(), 1);
        assert_eq!(input.discovery.harnesses[0].models.len(), 1);
        for key in STARTER_PROFILE_KEYS {
            let profile = &catalog.profiles[key];
            let metadata = profile.metadata.as_ref().unwrap();
            assert_eq!(profile.vendor.as_deref(), Some(provider));
            assert_eq!(metadata.candidates, vec![expected_candidate.clone()]);
            assert_eq!(metadata.fallback, None);
            assert_eq!(profile.escalate_to, None);
        }
        assert_eq!(catalog.roles, input.proposal.roles);
    }
}

#[test]
fn provisional_evidence_gap_applies_only_with_explicit_owner_acknowledgement() {
    // Given locally supported settings but no comparison evidence for recommendations.
    let (dir, store, mut input) = fixture();
    input.proposal.basis = SetupBasis::Provisional {
        exception: "Comparison evidence unavailable in fixture".into(),
    };
    input
        .proposal
        .recommendations
        .get_mut("database")
        .unwrap()
        .evidence
        .clear();
    let record = proposed(&store, &input);
    let before = fs::read(store.config_path()).unwrap();
    assert!(setup_approve(&store, &approval(&input), record.revision).is_err());
    assert!(setup_apply(&store).is_err());
    assert_eq!(store.profile_setup().unwrap(), record);
    assert_eq!(fs::read(store.config_path()).unwrap(), before);
    let mut receipt = approval(&input);
    receipt.acknowledged_evidence_gap = true;

    // When the exact evidence gap is explicitly acknowledged and applied.
    setup_approve(&store, &receipt, record.revision).unwrap();
    let applied = setup_apply(&store).unwrap();
    let reopened = Store::open_unobserved(dir.path()).unwrap();

    // Then the exception and acknowledgement persist without upgrading access claims.
    assert_eq!(applied.approval, Some(receipt));
    assert_eq!(applied.proposal, Some(input.proposal.clone()));
    assert_eq!(reopened.profile_setup().unwrap(), applied);
    assert!(!applied.validation.access_checked);
    assert!(!applied.validation.execution_tested);
}

#[test]
fn cancelled_setup_resumes_retained_merged_answers_without_applying() {
    // Given saved answers with an updated preference and a cancelled proposal.
    let (dir, store, input) = fixture();
    let before = fs::read(store.config_path()).unwrap();
    let gathering = setup_begin(&store, 0).unwrap();
    let initial = BTreeMap::from([
        ("provider".into(), "openai".into()),
        ("quality_cost".into(), "balanced".into()),
    ]);
    let saved = setup_answers(&store, &initial, gathering.revision).unwrap();
    let update = BTreeMap::from([("quality_cost".into(), "quality".into())]);
    let saved = setup_answers(&store, &update, saved.revision).unwrap();
    let expected_answers = BTreeMap::from([
        ("provider".into(), "openai".into()),
        ("quality_cost".into(), "quality".into()),
    ]);
    assert_eq!(saved.state, ProfileSetupState::Gathering);
    assert_eq!(saved.answers, expected_answers);
    let record = setup_propose(&store, &input, saved.revision).unwrap();
    let cancelled = setup_cancel(&store, record.revision).unwrap();
    assert!(setup_apply(&store).is_err());
    assert_eq!(fs::read(store.config_path()).unwrap(), before);
    let reopened = Store::open_unobserved(dir.path()).unwrap();

    // When a cancelled conversation resumes and is then proposed again.
    let resumed = setup_begin(&reopened, cancelled.revision).unwrap();
    let record = setup_propose(&reopened, &input, resumed.revision).unwrap();

    // Then retained answers require no re-entry and no mapping has been applied.
    assert_eq!(resumed.state, ProfileSetupState::Gathering);
    assert_eq!(resumed.answers, expected_answers);
    assert_eq!(resumed.proposal, None);
    assert_eq!(resumed.approval, None);
    assert_eq!(record.answers, expected_answers);
    assert_eq!(record.proposal, Some(input.proposal));
    assert_eq!(record.approval, None);
    assert_eq!(fs::read(reopened.config_path()).unwrap(), before);
}

#[test]
fn begin_is_idempotent_for_gathering_and_proposed_conversations() {
    // Given an in-progress conversation with persisted answers.
    let (_dir, store, input) = fixture();
    let gathering = setup_begin(&store, 0).unwrap();
    let answers = BTreeMap::from([("provider".into(), "openai".into())]);
    let gathering = setup_answers(&store, &answers, gathering.revision).unwrap();
    assert_eq!(setup_begin(&store, gathering.revision).unwrap(), gathering);
    let record = setup_propose(&store, &input, gathering.revision).unwrap();
    let history = store.profile_setup_history().unwrap();

    // When beginning again while a proposal awaits approval.
    let resumed = setup_begin(&store, record.revision).unwrap();

    // Then the exact proposal, revision, answers and journal remain intact.
    assert_eq!(resumed, record);
    assert_eq!(resumed.answers, answers);
    assert_eq!(store.profile_setup_history().unwrap(), history);
}

#[test]
fn approved_cross_provider_fallback_preserves_order_scope_cost_and_access_facets() {
    // Given three independently observed, metered candidates in owner-approved order.
    let (dir, store, input) = fixture();
    let input = metered_fallback_input(input);
    let expected = input.proposal.profiles["database"]
        .metadata
        .clone()
        .unwrap();
    let record = approved(&store, &input);
    assert_eq!(record.approval, Some(approval(&input)));

    // When applying and reopening the approved bounded fallback configuration.
    let applied = setup_apply(&store).unwrap();
    let reopened = Store::open_unobserved(dir.path()).unwrap();
    let catalog = setup_catalog(&reopened).unwrap();
    let actual = catalog.profiles["database"].metadata.as_ref().unwrap();

    // Then ordered exact candidates and all provider/spending bounds are unchanged.
    assert_eq!(actual.candidates, expected.candidates);
    assert_eq!(
        actual
            .candidates
            .iter()
            .map(|candidate| candidate.id.as_str())
            .collect::<Vec<_>>(),
        ["preferred", "second", "third"]
    );
    assert_eq!(actual.fallback, expected.fallback);
    assert_eq!(
        applied.proposal.as_ref().unwrap().preferences,
        input.proposal.preferences
    );
    assert_eq!(
        applied.proposal.as_ref().unwrap().access,
        input.proposal.access
    );
    assert!(applied.validation.configuration_valid);
    assert!(!applied.validation.access_checked);
    assert!(!applied.validation.execution_tested);
}

#[test]
fn fallback_outside_provider_or_cost_scope_cannot_publish_a_proposal() {
    // Given otherwise valid fallback candidates and one violated owner constraint.
    for case in [
        "provider_preference",
        "fallback_provider",
        "cost",
        "currency",
    ] {
        let (_dir, store, input) = fixture();
        let mut input = metered_fallback_input(input);
        match case {
            "provider_preference" => {
                input.proposal.preferences.allowed_providers = vec!["openai".into()];
            }
            "fallback_provider" => {
                input
                    .proposal
                    .profiles
                    .get_mut("database")
                    .unwrap()
                    .metadata
                    .as_mut()
                    .unwrap()
                    .fallback
                    .as_mut()
                    .unwrap()
                    .approved_providers
                    .remove("google");
            }
            "cost" | "currency" => {
                let candidate = &mut input
                    .proposal
                    .profiles
                    .get_mut("database")
                    .unwrap()
                    .metadata
                    .as_mut()
                    .unwrap()
                    .candidates[2];
                candidate.cost = CandidateCost::Metered {
                    currency: if case == "currency" { "EUR" } else { "USD" }.into(),
                    maximum_micros: if case == "cost" { 901 } else { 200 },
                };
            }
            _ => unreachable!(),
        }
        let gathering = setup_begin(&store, 0).unwrap();
        let before = fs::read(store.config_path()).unwrap();

        // When proposing a fallback outside that exact scope.
        let result = setup_propose(&store, &input, gathering.revision);

        // Then neither the proposal journal nor active mappings advance.
        assert!(result.is_err(), "{case}");
        assert_eq!(store.profile_setup().unwrap(), gathering, "{case}");
        assert_eq!(fs::read(store.config_path()).unwrap(), before, "{case}");
    }
}

#[test]
fn same_family_role_bindings_cannot_bypass_workspace_or_project_review_constraints() {
    // Given otherwise eligible role mappings and a required different-family reviewer.
    for project_scoped in [false, true] {
        for role in [ProfileRole::PlanReview, ProfileRole::ImplementationReview] {
            let (_dir, mut store, mut input) = fixture();
            let policy = ReviewOverride {
                require_cross_family: Some(true),
                ..Default::default()
            };
            if project_scoped {
                store.config.projects.insert("app".into(), project(policy));
            } else {
                store.config.review = policy;
            }
            store.save_config().unwrap();
            input.proposal.based_on_config_revision =
                store.config.profile_setup_revision().unwrap();
            input.proposal.roles = BTreeMap::from([
                (ProfileRole::Implementation, "database".into()),
                (ProfileRole::Planning, "database".into()),
                (role, "database".into()),
            ]);
            input
                .proposal
                .profiles
                .get_mut("database")
                .unwrap()
                .metadata
                .as_mut()
                .unwrap()
                .eligibility
                .roles
                .extend([ProfileRole::Implementation, role]);
            input.proposal.validate(&store.config).unwrap();
            let gathering = setup_begin(&store, 0).unwrap();
            let before = fs::read(store.config_path()).unwrap();

            // When proposing the same provider for authors and a review role.
            let result = setup_propose(&store, &input, gathering.revision);

            // Then eligibility alone cannot override either policy scope.
            assert!(result.is_err(), "{project_scoped}/{role:?}");
            assert_eq!(store.profile_setup().unwrap(), gathering);
            assert_eq!(fs::read(store.config_path()).unwrap(), before);
        }
    }
}

#[test]
fn custom_catalog_retains_examples_overlap_owner_rules_and_effective_review_policies() {
    // Given overlapping custom profiles, ordered owner rules and project review overrides.
    let (dir, mut store, mut input) = fixture();
    store.config.routing.rules = vec![
        RoutingRule {
            name: Some("owner-first".into()),
            project: vec!["app".into()],
            kind: vec![],
            size: vec![],
            title_regex: Some("migration".into()),
            body_regex: None,
            profile: "omo".into(),
        },
        RoutingRule {
            name: Some("owner-catch-all".into()),
            project: vec![],
            kind: vec![],
            size: vec![],
            title_regex: None,
            body_regex: None,
            profile: "claude".into(),
        },
    ];
    store.config.review.owner_paths = Some(vec!["secrets/**".into()]);
    store.config.projects.insert(
        "app".into(),
        project(ReviewOverride {
            plan_required: Some(true),
            max_rounds: Some(7),
            ..Default::default()
        }),
    );
    store.save_config().unwrap();
    input.proposal.based_on_config_revision = store.config.profile_setup_revision().unwrap();
    let mut custom = input.proposal.profiles["database"].clone();
    let metadata = custom.metadata.as_mut().unwrap();
    metadata.revision = "schema-r1".into();
    metadata.display_name = "Schema".into();
    metadata.guidance.positive_examples = vec!["Add indexed migration".into()];
    metadata.guidance.negative_examples = vec!["Rewrite release notes".into()];
    metadata.guidance.distinctions = vec!["Use Database for transaction semantics".into()];
    input.proposal.profiles.insert("schema".into(), custom);
    input.proposal.recommendations.insert(
        "schema".into(),
        input.proposal.recommendations["database"].clone(),
    );
    input.proposal.access.push(CandidateAccess {
        profile: "schema".into(),
        ..input.proposal.access[0].clone()
    });
    let owner_rules = store.config.routing.rules.clone();
    let policies = BTreeMap::from([
        ("workspace".into(), store.config.review_policy(None, None)),
        (
            "project:app".into(),
            store.config.review_policy(Some("app"), None),
        ),
    ]);
    approved(&store, &input);

    // When the custom catalog is applied and inspected after restart.
    setup_apply(&store).unwrap();
    let reopened = Store::open_unobserved(dir.path()).unwrap();
    let catalog = setup_catalog(&reopened).unwrap();

    // Then exact guidance and overlap coexist with visible unchanged owner constraints.
    for key in ["database", "schema"] {
        assert_eq!(catalog.profiles[key], input.proposal.profiles[key]);
    }
    assert_eq!(
        catalog.profiles["database"]
            .metadata
            .as_ref()
            .unwrap()
            .guidance
            .use_when,
        catalog.profiles["schema"]
            .metadata
            .as_ref()
            .unwrap()
            .guidance
            .use_when
    );
    assert_eq!(catalog.owner_rules, owner_rules);
    assert_eq!(catalog.review_policies, policies);
    assert_eq!(reopened.config.routing.rules, owner_rules);
}
