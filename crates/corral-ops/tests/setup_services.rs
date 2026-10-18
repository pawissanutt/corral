use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use corral_core::*;
use corral_ops::*;

#[path = "setup_services/rejections.rs"]
mod rejections;
#[path = "setup_services/support_persistence.rs"]
mod support_persistence;

fn now() -> jiff::Timestamp {
    "2026-10-18T12:00:00Z".parse().unwrap()
}

fn fixture() -> (tempfile::TempDir, Store, SetupProposalInput) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(dir.path()).unwrap();
    store.config.projects.clear();
    store.save_config().unwrap();
    let evidence = ModelEvidence {
        version: 1,
        id: "local-support".into(),
        source: ModelEvidenceSource::LocalDiscovery {
            method: "isolated fixture capability listing".into(),
        },
        retrieved_at: now(),
        published_on: None,
        measured_on: None,
        claim: "Exact fixture supports high effort".into(),
        finding: "Local supported configuration and owner-reported connection".into(),
        limitations: vec!["No execution or account probe".into()],
    };
    let candidate = ExecutionCandidate {
        id: "preferred".into(),
        harness: "omo".into(),
        connection: "owner-local".into(),
        provider: "openai".into(),
        model: "openai/fixture-v1".into(),
        effort: Some("high".into()),
        args: vec![],
        evidence: evidence.id.clone(),
        cost: CandidateCost::Subscription,
    };
    let facets = AccessFacets {
        installed: true,
        discovered: true,
        user_reported: true,
        access_verified: false,
        execution_tested: false,
    };
    let observation = CandidateObservation {
        harness: candidate.harness.clone(),
        connection: candidate.connection.clone(),
        provider: candidate.provider.clone(),
        model: candidate.model.clone(),
        observed_at: now(),
        facets: facets.clone(),
        settings: vec![CandidateSettings {
            effort: candidate.effort.clone(),
            args: vec![],
        }],
        evidence: vec![evidence.clone()],
    };
    let profile = Profile {
        harness: candidate.harness.clone(),
        model: Some(candidate.model.clone()),
        vendor: Some(candidate.provider.clone()),
        effort: candidate.effort.clone(),
        args: vec![],
        escalate_to: None,
        metadata: Some(ProfileMetadata {
            display_name: "Database".into(),
            purpose: "Transaction correctness".into(),
            guidance: SelectionGuidance {
                use_when: vec!["Transaction boundaries dominate".into()],
                avoid_when: vec!["Only prose changes".into()],
                positive_examples: vec!["Repair rollback".into()],
                negative_examples: vec!["Document SQL".into()],
                distinctions: vec!["Prefer Writing for prose".into()],
            },
            instructions: "Trace persistence contracts".into(),
            eligibility: ProfileEligibility {
                enabled: true,
                automatic: true,
                roles: BTreeSet::from([ProfileRole::Implementation, ProfileRole::Planning]),
                projects: BTreeSet::new(),
            },
            revision: "database-r1".into(),
            evidence: Some(evidence.id.clone()),
            candidates: vec![candidate],
            fallback: None,
        }),
    };
    let proposal = ProfileSetupProposal {
        version: 1,
        id: "setup-1".into(),
        prepared_at: now(),
        procedure_version: "fixture-procedure-v1".into(),
        based_on_config_revision: store.config.profile_setup_revision().unwrap(),
        preferences: SetupPreferences::default(),
        basis: SetupBasis::Sourced,
        profiles: BTreeMap::from([("database".into(), profile)]),
        roles: BTreeMap::from([(ProfileRole::Planning, "database".into())]),
        evidence: BTreeMap::from([(evidence.id.clone(), evidence)]),
        recommendations: BTreeMap::from([(
            "database".into(),
            Recommendation {
                reason: "Owner-selected fixture for persistence".into(),
                evidence: vec!["local-support".into()],
                alternatives_considered: vec![],
                uncertainty: vec!["Execution untested".into()],
            },
        )]),
        access: vec![CandidateAccess {
            profile: "database".into(),
            candidate: "preferred".into(),
            observed_at: now(),
            facets,
            evidence: vec!["local-support".into()],
        }],
        repair_policy: None,
    };
    (
        dir,
        store,
        SetupProposalInput {
            proposal,
            discovery: SetupDiscovery {
                observed_at: now(),
                harnesses: vec![DiscoveredHarness {
                    key: "omo".into(),
                    installed: true,
                    models: vec!["openai/fixture-v1".into()],
                    efforts: vec!["high".into()],
                }],
                candidates: vec![observation],
            },
        },
    )
}

fn approval(input: &SetupProposalInput) -> ProfileSetupApproval {
    ProfileSetupApproval {
        version: 1,
        id: format!("approval-{}", input.proposal.id),
        by: SetupApprover::Owner,
        at: now(),
        proposal: input.proposal.id.clone(),
        proposal_digest: input.proposal.digest().unwrap(),
        based_on_config_revision: input.proposal.based_on_config_revision.clone(),
        acknowledged_evidence_gap: false,
    }
}

fn proposed(store: &Store, input: &SetupProposalInput) -> ProfileSetupRecord {
    let current = store.profile_setup().unwrap();
    let gathering = setup_begin(store, current.revision).unwrap();
    setup_propose(store, input, gathering.revision).unwrap()
}

fn approved(store: &Store, input: &SetupProposalInput) -> ProfileSetupRecord {
    let proposed = proposed(store, input);
    setup_approve(store, &approval(input), proposed.revision).unwrap()
}

#[test]
fn approved_setup_persists_after_restart_and_preserves_authority() {
    // Given an exact proposal, independent access facets and owner approval.
    let (dir, store, input) = fixture();
    let authority = store.config.automation.clone();
    approved(&store, &input);
    let before_events = fs::read(store.dir().join("events.jsonl")).unwrap();
    let status = setup_status(&store).unwrap();
    let diff = setup_diff(&store).unwrap();
    assert_eq!(status.record.state, ProfileSetupState::Proposed);
    assert!(!serde_json::to_value(diff).unwrap().is_null());
    assert_eq!(
        before_events,
        fs::read(store.dir().join("events.jsonl")).unwrap()
    );
    // When applying and reopening the real store.
    let applied = setup_apply(&store).unwrap();
    let reopened = Store::open_unobserved(dir.path()).unwrap();
    // Then exact bindings, approval and catalog guidance persist, without invented verification.
    assert_eq!(
        reopened.config.profiles["database"],
        input.proposal.profiles["database"]
    );
    assert_eq!(
        reopened.config.routing.role_profiles[&ProfileRole::Planning],
        "database"
    );
    assert_eq!(reopened.config.automation, authority);
    assert!(reopened.list().unwrap().is_empty());
    assert_eq!(applied.state, ProfileSetupState::Applied);
    assert!(!applied.validation.access_checked);
    assert!(!applied.validation.execution_tested);
    assert_eq!(setup_apply(&reopened).unwrap(), applied);
    let catalog = setup_catalog(&reopened).unwrap();
    assert_eq!(
        catalog.profiles["database"]
            .metadata
            .as_ref()
            .unwrap()
            .guidance,
        input.proposal.profiles["database"]
            .metadata
            .as_ref()
            .unwrap()
            .guidance
    );
}

#[test]
fn custom_profile_revision_uses_same_approval_path_as_initial_setup() {
    // Given an applied custom profile.
    let (dir, store, mut input) = fixture();
    approved(&store, &input);
    setup_apply(&store).unwrap();
    let reopened = Store::open_unobserved(dir.path()).unwrap();
    let before_catalog = setup_catalog(&reopened).unwrap().revision;
    input.proposal.id = "setup-2".into();
    input.proposal.based_on_config_revision = reopened.config.profile_setup_revision().unwrap();
    let metadata = input
        .proposal
        .profiles
        .get_mut("database")
        .unwrap()
        .metadata
        .as_mut()
        .unwrap();
    metadata.revision = "database-r2".into();
    metadata.guidance.use_when = vec!["Only migrations".into()];
    let proposed = proposed(&reopened, &input);
    assert!(setup_apply(&reopened).is_err());
    // When approving the exact targeted revision and applying it.
    setup_approve(&reopened, &approval(&input), proposed.revision).unwrap();
    setup_apply(&reopened).unwrap();
    // Then the current catalog changes and both approvals remain in durable history.
    let catalog = setup_catalog(&reopened).unwrap();
    assert_ne!(catalog.revision, before_catalog);
    assert_eq!(
        catalog.profiles["database"],
        input.proposal.profiles["database"]
    );
    assert_eq!(
        reopened
            .profile_setup_history()
            .unwrap()
            .iter()
            .filter(|record| record.state == ProfileSetupState::Applied)
            .count(),
        2
    );
}
