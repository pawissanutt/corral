use std::collections::{BTreeMap, BTreeSet};

use corral_core::*;

pub fn now() -> jiff::Timestamp {
    "2026-10-18T12:00:00Z".parse().unwrap()
}

pub fn profile(provider: &str) -> Profile {
    Profile {
        harness: "omo".into(),
        model: Some(format!("{provider}/fixture-v1")),
        vendor: Some(provider.into()),
        effort: Some("high".into()),
        args: vec![],
        escalate_to: None,
        metadata: Some(ProfileMetadata {
            display_name: "Fixture".into(),
            purpose: "Deterministic setup".into(),
            guidance: SelectionGuidance::default(),
            instructions: String::new(),
            eligibility: ProfileEligibility {
                enabled: true,
                automatic: true,
                roles: BTreeSet::from([
                    ProfileRole::Implementation,
                    ProfileRole::Planning,
                    ProfileRole::Pm,
                    ProfileRole::PlanReview,
                    ProfileRole::ImplementationReview,
                ]),
                projects: BTreeSet::new(),
            },
            revision: "fixture-r1".into(),
            evidence: Some("e1".into()),
            candidates: vec![ExecutionCandidate {
                id: "preferred".into(),
                harness: "omo".into(),
                connection: "fixture-local".into(),
                provider: provider.into(),
                model: format!("{provider}/fixture-v1"),
                effort: Some("high".into()),
                args: vec![],
                evidence: "e1".into(),
                cost: CandidateCost::Subscription,
            }],
            fallback: None,
        }),
    }
}

pub fn proposal(config: &Config) -> ProfileSetupProposal {
    ProfileSetupProposal {
        version: 1,
        id: "p1".into(),
        prepared_at: now(),
        procedure_version: "fixture-v1".into(),
        based_on_config_revision: config.profile_setup_revision().unwrap(),
        preferences: SetupPreferences::default(),
        basis: SetupBasis::Sourced,
        profiles: BTreeMap::from([("custom".into(), profile("openai"))]),
        roles: BTreeMap::from([(ProfileRole::Implementation, "custom".into())]),
        evidence: BTreeMap::from([(
            "e1".into(),
            ModelEvidence {
                version: 1,
                id: "e1".into(),
                source: ModelEvidenceSource::LocalDiscovery {
                    method: "deterministic fixture".into(),
                },
                retrieved_at: now(),
                published_on: None,
                measured_on: None,
                claim: "Exact fixture binding".into(),
                finding: "Available in fixture".into(),
                limitations: vec![],
            },
        )]),
        recommendations: BTreeMap::from([(
            "custom".into(),
            Recommendation {
                reason: "Owner fixture choice".into(),
                evidence: vec!["e1".into()],
                alternatives_considered: vec![],
                uncertainty: vec![],
            },
        )]),
        access: vec![],
        repair_policy: Some(RepairPolicy::approved_starter_policy()),
    }
}

pub fn approval(proposal: &ProfileSetupProposal) -> ProfileSetupApproval {
    ProfileSetupApproval {
        version: 1,
        id: "a1".into(),
        by: SetupApprover::Owner,
        at: now(),
        proposal: proposal.id.clone(),
        proposal_digest: proposal.digest().unwrap(),
        based_on_config_revision: proposal.based_on_config_revision.clone(),
        acknowledged_evidence_gap: false,
    }
}

pub fn proposed(store: &Store, proposal: ProfileSetupProposal) -> ProfileSetupRecord {
    store
        .save_profile_setup(&ProfileSetupRecord {
            revision: 1,
            state: ProfileSetupState::Gathering,
            ..Default::default()
        })
        .unwrap();
    let record = ProfileSetupRecord {
        revision: 2,
        state: ProfileSetupState::Proposed,
        approval: Some(approval(&proposal)),
        proposal: Some(proposal),
        ..Default::default()
    };
    store.save_profile_setup(&record).unwrap();
    record
}

pub fn fixture() -> (tempfile::TempDir, Store, ProfileSetupRecord) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let record = proposed(&store, proposal(&store.config));
    (dir, store, record)
}

pub fn valid(_: &ProfileSetupProposal, _: &Config) -> anyhow::Result<SetupValidationFacets> {
    Ok(SetupValidationFacets {
        configuration_valid: true,
        access_checked: false,
        execution_tested: false,
    })
}
