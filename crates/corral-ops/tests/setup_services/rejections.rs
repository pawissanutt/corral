use super::*;

#[test]
fn invalid_candidates_do_not_persist_proposal_or_change_config() {
    // Given independent invalid candidates and a gathering state.
    for case in [
        "settings",
        "connection",
        "access",
        "future_evidence",
        "manual",
        "argv",
    ] {
        let (_dir, store, mut input) = fixture();
        let gathering = setup_begin(&store, 0).unwrap();
        let config = fs::read(store.config_path()).unwrap();
        match case {
            "settings" => input.discovery.candidates[0].settings.clear(),
            "connection" => input.discovery.candidates[0].connection = "other".into(),
            "access" => {
                input.discovery.candidates[0].facets.user_reported = false;
                input.proposal.access[0].facets.user_reported = false;
            }
            "future_evidence" => {
                input
                    .proposal
                    .evidence
                    .get_mut("local-support")
                    .unwrap()
                    .retrieved_at = "2026-10-19T12:00:00Z".parse().unwrap()
            }
            "manual" => input.proposal.basis = SetupBasis::Manual,
            "argv" => {
                input.proposal.profiles.get_mut("database").unwrap().args =
                    vec!["--model=other".into()];
                input
                    .proposal
                    .profiles
                    .get_mut("database")
                    .unwrap()
                    .metadata
                    .as_mut()
                    .unwrap()
                    .candidates[0]
                    .args = vec!["--model=other".into()];
                input.discovery.candidates[0].settings[0].args = vec!["--model=other".into()];
            }
            _ => unreachable!(),
        }
        // When proposing an invalid configuration.
        let result = setup_propose(&store, &input, gathering.revision);
        // Then neither the persisted proposal nor active mapping changes.
        assert!(result.is_err(), "{case}");
        assert_eq!(store.profile_setup().unwrap(), gathering, "{case}");
        assert_eq!(fs::read(store.config_path()).unwrap(), config, "{case}");
    }
}

fn add_fallback(input: &mut SetupProposalInput) {
    input.discovery.harnesses.push(DiscoveredHarness {
        key: "claude".into(),
        installed: true,
        models: vec!["anthropic/fixture-fallback-v1".into()],
        efforts: vec!["medium".into()],
    });
    let metadata = input
        .proposal
        .profiles
        .get_mut("database")
        .unwrap()
        .metadata
        .as_mut()
        .unwrap();
    let mut candidate = metadata.candidates[0].clone();
    candidate.id = "fallback".into();
    candidate.harness = "claude".into();
    candidate.provider = "anthropic".into();
    candidate.connection = "owner-anthropic".into();
    candidate.model = "anthropic/fixture-fallback-v1".into();
    candidate.effort = Some("medium".into());
    metadata.candidates.push(candidate.clone());
    metadata.fallback = Some(AvailabilityFallback {
        automatic: true,
        approved_providers: BTreeSet::from(["openai".into(), "anthropic".into()]),
        max_switches: 1,
        allow_unknown_subscription_cost: true,
        spending: None,
        approval: input.proposal.id.clone(),
    });
    let mut observation = input.discovery.candidates[0].clone();
    observation.harness = candidate.harness;
    observation.provider = candidate.provider;
    observation.connection = candidate.connection;
    observation.model = candidate.model;
    observation.settings[0].effort = Some("medium".into());
    input.discovery.candidates.push(observation);
    let mut access = input.proposal.access[0].clone();
    access.candidate = "fallback".into();
    input.proposal.access.push(access);
}

#[test]
fn every_fallback_requires_its_own_settings_access_provider_and_cost_permission() {
    // Given a valid independently observed cross-provider fallback.
    for case in [
        "valid",
        "settings",
        "connection",
        "provider",
        "unknown_cost",
        "cap",
    ] {
        let (_dir, store, mut input) = fixture();
        add_fallback(&mut input);
        match case {
            "valid" => {}
            "settings" => input.discovery.candidates[1].settings[0].effort = Some("high".into()),
            "connection" => input.discovery.candidates[1].connection = "not-approved".into(),
            "provider" => {
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
                    .remove("anthropic");
            }
            "unknown_cost" => {
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
                    .allow_unknown_subscription_cost = false;
            }
            "cap" => {
                input.proposal.preferences.spending = Some(SpendingBound {
                    currency: "USD".into(),
                    maximum_micros: 1,
                })
            }
            _ => unreachable!(),
        }
        // When validating the whole proposal.
        let result = setup_validate(&store, &input);
        // Then the preferred model never masks an invalid fallback.
        assert_eq!(result.is_ok(), case == "valid", "{case}: {result:?}");
    }
}

#[test]
fn approval_requires_exact_digest_and_provisional_acknowledgement() {
    // Given a provisional proposal with an explicit evidence exception.
    let (_dir, store, mut input) = fixture();
    input.proposal.basis = SetupBasis::Provisional {
        exception: "Comparison source unavailable".into(),
    };
    let proposed = proposed(&store, &input);
    let before = fs::read(store.config_path()).unwrap();
    let mut receipt = approval(&input);
    // When approval omits acknowledgement or binds a different proposal.
    assert!(setup_approve(&store, &receipt, proposed.revision).is_err());
    assert_eq!(store.profile_setup().unwrap(), proposed);
    receipt.acknowledged_evidence_gap = true;
    receipt.proposal_digest = "not-the-exact-proposal".into();
    assert!(setup_approve(&store, &receipt, proposed.revision).is_err());
    // Then nothing has applied; exact acknowledged approval persists and allows apply.
    assert_eq!(fs::read(store.config_path()).unwrap(), before);
    receipt.proposal_digest = input.proposal.digest().unwrap();
    setup_approve(&store, &receipt, proposed.revision).unwrap();
    let applied = setup_apply(&store).unwrap();
    assert!(applied.approval.unwrap().acknowledged_evidence_gap);
}

#[test]
fn concurrent_relevant_edit_rejects_apply_without_restoring_old_config() {
    // Given approved setup and another writer's relevant guidance change.
    let (_dir, store, input) = fixture();
    let approved = approved(&store, &input);
    store
        .edit_config(|doc| doc.set_str("profiles.omo.model", "openai/changed-model"))
        .unwrap();
    let before = fs::read(store.config_path()).unwrap();
    // When applying the now-stale proposal through the original Store handle.
    let result = setup_apply(&store);
    // Then both the newer config and the proposed journal state remain intact.
    assert!(result.is_err());
    assert_eq!(fs::read(store.config_path()).unwrap(), before);
    assert_eq!(store.profile_setup().unwrap(), approved);
}

#[test]
fn cancelled_proposal_never_changes_mapping_and_stale_revision_cannot_approve() {
    // Given a proposed configuration that was cancelled.
    let (_dir, store, input) = fixture();
    let proposed = proposed(&store, &input);
    let before = fs::read(store.config_path()).unwrap();
    let cancelled = setup_cancel(&store, proposed.revision).unwrap();
    // When old approval/apply calls are redelivered.
    assert!(setup_approve(&store, &approval(&input), proposed.revision).is_err());
    assert!(setup_apply(&store).is_err());
    // Then the cancellation persists without touching active mappings.
    assert_eq!(store.profile_setup().unwrap(), cancelled);
    assert_eq!(fs::read(store.config_path()).unwrap(), before);
}
