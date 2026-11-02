//! The built-in corral-pi host takes no `model_args`: setup represents it through its own rules.
use super::*;

/// Rebinds the fixture's `database` profile, candidate, observation and inventory to corral-pi.
fn corral_pi(input: &mut SetupProposalInput, vendor: &str, model: &str) {
    let profile = input.proposal.profiles.get_mut("database").unwrap();
    profile.harness = CORRAL_PI_HARNESS.into();
    profile.model = Some(model.into());
    profile.vendor = Some(vendor.into());
    profile.effort = Some("medium".into());
    let candidate = &mut profile.metadata.as_mut().unwrap().candidates[0];
    candidate.harness = CORRAL_PI_HARNESS.into();
    candidate.provider = vendor.into();
    candidate.model = model.into();
    candidate.effort = Some("medium".into());
    let observation = &mut input.discovery.candidates[0];
    observation.harness = CORRAL_PI_HARNESS.into();
    observation.provider = vendor.into();
    observation.model = model.into();
    observation.settings[0].effort = Some("medium".into());
    input.discovery.harnesses = vec![DiscoveredHarness {
        key: CORRAL_PI_HARNESS.into(),
        installed: true,
        models: vec![],
        efforts: vec!["medium".into()],
        vendors: CORRAL_PI_VENDORS.map(String::from).into(),
    }];
}

#[test]
fn openai_corral_pi_profile_validates_previews_approves_and_applies() {
    // Given a corral-pi profile on the allowed vendor, with no model_args anywhere.
    let (dir, store, mut input) = fixture();
    corral_pi(&mut input, "openai", "gpt-6.1-sol");
    // When it goes through propose, diff, approve and apply.
    approved(&store, &input);
    assert!(
        !serde_json::to_value(setup_diff(&store).unwrap())
            .unwrap()
            .is_null()
    );
    let applied = setup_apply(&store).unwrap();
    // Then the exact corral-pi binding persists.
    assert_eq!(applied.state, ProfileSetupState::Applied);
    let reopened = Store::open_unobserved(dir.path()).unwrap();
    assert_eq!(
        reopened.config.profiles["database"],
        input.proposal.profiles["database"]
    );
}

#[test]
fn non_openai_corral_pi_profile_is_refused_with_vendor_refused() {
    // Given a corral-pi profile on a vendor outside the allowlist.
    let (_dir, store, mut input) = fixture();
    corral_pi(&mut input, "mistral", "mistral-large");
    let gathering = setup_begin(&store, store.profile_setup().unwrap().revision).unwrap();
    // When proposing, then validation refuses it by the shared vendor rule.
    let error = format!(
        "{:#}",
        setup_propose(&store, &input, gathering.revision).unwrap_err()
    );
    assert!(error.contains("vendor_refused"), "{error}");
}

#[test]
fn corral_pi_candidate_with_args_or_unknown_effort_is_refused() {
    // Given corral-pi candidates that carry argv, or an effort its flag map lacks.
    for (args, effort, expected) in [
        (
            vec!["--max-tokens=1000".to_owned()],
            "medium",
            "takes no args",
        ),
        (vec![], "extreme", "unknown effort"),
    ] {
        let (_dir, store, mut input) = fixture();
        corral_pi(&mut input, "openai", "gpt-6.1-sol");
        let metadata = input
            .proposal
            .profiles
            .get_mut("database")
            .unwrap()
            .metadata
            .as_mut()
            .unwrap();
        metadata.candidates[0].args = args.clone();
        metadata.candidates[0].effort = Some(effort.into());
        let gathering = setup_begin(&store, store.profile_setup().unwrap().revision).unwrap();
        // When proposing, then the corral-pi rules reject it.
        let error = format!(
            "{:#}",
            setup_propose(&store, &input, gathering.revision).unwrap_err()
        );
        assert!(error.contains(expected), "{error}");
    }
}
