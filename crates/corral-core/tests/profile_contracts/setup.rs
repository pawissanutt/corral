use super::*;

#[test]
fn sourced_setup_round_trips_independent_dates_access_and_approval() {
    // Given a sourced proposal with discovery but no access/execution verification.
    let (_dir, config) = config();
    let proposed = proposal();
    let approved = approval(&proposed);
    // When JSON and YAML round-trip the proposal and approval.
    let decoded: ProfileSetupProposal =
        serde_json::from_str(&serde_json::to_string(&proposed).unwrap()).unwrap();
    let decoded_approval: ProfileSetupApproval =
        serde_yaml_ng::from_str(&serde_yaml_ng::to_string(&approved).unwrap()).unwrap();
    // Then dates, access facets and the exact approved choices remain distinct.
    assert_eq!(decoded, proposed);
    assert_ne!(
        decoded.evidence["evidence-1"].published_on,
        decoded.evidence["evidence-1"].measured_on
    );
    assert!(decoded.access[0].facets.discovered);
    assert!(!decoded.access[0].facets.access_verified);
    assert!(!decoded.access[0].facets.execution_tested);
    assert!(decoded.validate(&config).is_ok());
    assert!(decoded.check_approval(&decoded_approval).is_ok());
}

#[test]
fn approval_binds_exact_proposal_and_configuration_revision() {
    // Given owner approval of one exact proposal.
    for case in ["model", "revision", "role", "guidance"] {
        let mut proposed = proposal();
        let approved = approval(&proposed);
        // When any approved choice/revision is changed.
        match case {
            "model" => {
                proposed.profiles.get_mut("custom.database").unwrap().model = Some("other".into())
            }
            "revision" => proposed.based_on_config_revision = "config-r2".into(),
            "role" => proposed.roles.clear(),
            "guidance" => proposed
                .profiles
                .get_mut("custom.database")
                .unwrap()
                .metadata
                .as_mut()
                .unwrap()
                .guidance
                .use_when
                .clear(),
            _ => unreachable!(),
        }
        // Then the old owner approval cannot authorize that proposal.
        assert!(proposed.check_approval(&approved).is_err(), "{case}");
    }
}

#[test]
fn provisional_exception_requires_explicit_owner_acknowledgement() {
    // Given a provisional mapping with recorded evidence limitation.
    let (_dir, config) = config();
    let mut proposed = proposal();
    proposed.basis = SetupBasis::Provisional {
        exception: "Comparison site inaccessible".into(),
    };
    let mut approved = approval(&proposed);
    // When checking an approval without evidence-gap acknowledgement.
    let rejected = proposed.check_approval(&approved);
    // Then it is rejected; explicit acknowledgement permits this exact proposal.
    assert!(proposed.validate(&config).is_ok());
    assert!(rejected.is_err());
    approved.acknowledged_evidence_gap = true;
    assert!(proposed.check_approval(&approved).is_ok());
}

#[test]
fn setup_rejects_broken_evidence_role_and_escalation_references() {
    // Given separate malformed proposal references.
    let (_dir, config) = config();
    for case in [
        "evidence",
        "role",
        "candidate",
        "cycle",
        "version",
        "recommendation",
    ] {
        let mut proposed = proposal();
        match case {
            "evidence" => proposed.evidence.clear(),
            "role" => {
                proposed.roles.insert(ProfileRole::Pm, "missing".into());
            }
            "candidate" => proposed.access[0].candidate = "missing".into(),
            "cycle" => {
                proposed
                    .profiles
                    .get_mut("custom.database")
                    .unwrap()
                    .escalate_to = Some("custom.database".into())
            }
            "version" => proposed.evidence.get_mut("evidence-1").unwrap().version = 9,
            "recommendation" => proposed.recommendations.clear(),
            _ => unreachable!(),
        }
        // When validating the proposed catalog.
        let result = proposed.validate(&config);
        // Then no broken reference reaches apply.
        assert!(result.is_err(), "{case}");
    }
}

#[test]
fn setup_state_machine_rejects_skipped_approval_stale_revision_and_rewritten_apply() {
    // Given a gathering state and a valid approved proposed state.
    let (_dir, config) = config();
    let gathering = ProfileSetupRecord {
        revision: 1,
        state: ProfileSetupState::Gathering,
        ..Default::default()
    };
    let proposed = proposal();
    let approved = ProfileSetupRecord {
        revision: 2,
        state: ProfileSetupState::Proposed,
        approval: Some(approval(&proposed)),
        proposal: Some(proposed),
        ..Default::default()
    };
    assert!(gathering.check_transition(&approved).is_ok());
    // When attempting invalid state/revision transitions.
    let applied = ProfileSetupRecord {
        revision: 3,
        state: ProfileSetupState::Applied,
        applied_config_revision: Some("config-r2".into()),
        validation: SetupValidationFacets {
            configuration_valid: true,
            ..Default::default()
        },
        ..approved.clone()
    };
    // Then only exact, approved apply is valid.
    assert!(approved.check_transition(&applied).is_ok());
    assert!(applied.validate(&config).is_ok());
    assert!(gathering.check_transition(&applied).is_err());
    assert!(approved.check_transition(&approved).is_err());
    let mut rewritten = applied.clone();
    rewritten
        .proposal
        .as_mut()
        .unwrap()
        .preferences
        .allowed_providers
        .push("unapproved".into());
    assert!(approved.check_transition(&rewritten).is_err());
    let mut no_approval = applied;
    no_approval.approval = None;
    assert!(no_approval.validate(&config).is_err());
}

#[test]
fn manual_mapping_is_not_falsely_labelled_sourced() {
    // Given explicit manual metadata without evidence claims.
    let (_dir, config) = config();
    let mut proposed = proposal();
    proposed.basis = SetupBasis::Manual;
    proposed.evidence.clear();
    let m = proposed
        .profiles
        .get_mut("custom.database")
        .unwrap()
        .metadata
        .as_mut()
        .unwrap();
    m.evidence = None;
    m.candidates.clear();
    proposed
        .recommendations
        .get_mut("custom.database")
        .unwrap()
        .evidence
        .clear();
    proposed.access.clear();
    // When validating a manual proposal and the same claims as sourced.
    assert!(proposed.validate(&config).is_ok());
    proposed.basis = SetupBasis::Sourced;
    // Then missing sources cannot be relabelled as researched.
    assert!(proposed.validate(&config).is_err());
}
