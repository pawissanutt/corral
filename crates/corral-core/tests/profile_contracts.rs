use std::collections::{BTreeMap, BTreeSet};

use corral_core::*;

#[path = "profile_contracts/catalog.rs"]
mod catalog;
#[path = "profile_contracts/review_regressions.rs"]
mod review_regressions;
#[path = "profile_contracts/selection.rs"]
mod selection;
#[path = "profile_contracts/setup.rs"]
mod setup;
#[path = "profile_contracts/store.rs"]
mod store;

fn now() -> jiff::Timestamp {
    "2026-10-18T12:00:00Z".parse().unwrap()
}

fn candidate(id: &str) -> ExecutionCandidate {
    ExecutionCandidate {
        id: id.into(),
        harness: "omo".into(),
        connection: "owner-connection".into(),
        provider: "openai".into(),
        model: "openai/exact-fixture-v1".into(),
        effort: Some("high".into()),
        args: vec![],
        evidence: "evidence-1".into(),
        cost: CandidateCost::Subscription,
    }
}

fn metadata() -> ProfileMetadata {
    ProfileMetadata {
        display_name: "Database".into(),
        purpose: "Persistence correctness".into(),
        guidance: SelectionGuidance {
            use_when: vec!["Transaction boundaries dominate".into()],
            avoid_when: vec!["Only database prose changes".into()],
            positive_examples: vec!["Repair transaction rollback".into()],
            negative_examples: vec!["Document SQL syntax".into()],
            distinctions: vec!["Select Writing for prose".into()],
        },
        instructions: "Trace rollback and migration contracts".into(),
        eligibility: ProfileEligibility {
            enabled: true,
            automatic: true,
            roles: BTreeSet::from([
                ProfileRole::Implementation,
                ProfileRole::Planning,
                ProfileRole::PlanReview,
                ProfileRole::ImplementationReview,
            ]),
            projects: BTreeSet::new(),
        },
        revision: "profile-r1".into(),
        evidence: Some("evidence-1".into()),
        candidates: vec![candidate("preferred")],
        fallback: None,
    }
}

fn custom_profile() -> Profile {
    Profile {
        metadata: Some(metadata()),
        harness: "omo".into(),
        model: Some("openai/exact-fixture-v1".into()),
        vendor: Some("openai".into()),
        effort: Some("high".into()),
        args: vec![],
        escalate_to: None,
    }
}

fn config() -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    config
        .profiles
        .insert("custom.database".into(), custom_profile());
    (dir, config)
}

fn proposal() -> ProfileSetupProposal {
    ProfileSetupProposal {
        version: 1,
        id: "proposal-1".into(),
        prepared_at: now(),
        procedure_version: "model-selection-v1".into(),
        based_on_config_revision: "config-r1".into(),
        preferences: SetupPreferences::default(),
        basis: SetupBasis::Sourced,
        profiles: BTreeMap::from([("custom.database".into(), custom_profile())]),
        roles: BTreeMap::from([(ProfileRole::Planning, "custom.database".into())]),
        evidence: BTreeMap::from([(
            "evidence-1".into(),
            ModelEvidence {
                version: 1,
                id: "evidence-1".into(),
                source: ModelEvidenceSource::LocalDiscovery {
                    method: "fixture models --json".into(),
                },
                retrieved_at: now(),
                published_on: Some("2026-10-01".parse().unwrap()),
                measured_on: Some("2026-09-20".parse().unwrap()),
                claim: "Runnable exact fixture".into(),
                finding: "Listed by harness".into(),
                limitations: vec!["No comparative claim".into()],
            },
        )]),
        recommendations: BTreeMap::from([(
            "custom.database".into(),
            Recommendation {
                reason: "Owner-selected local fixture".into(),
                evidence: vec!["evidence-1".into()],
                alternatives_considered: vec![],
                uncertainty: vec!["Execution untested".into()],
            },
        )]),
        access: vec![CandidateAccess {
            profile: "custom.database".into(),
            candidate: "preferred".into(),
            observed_at: now(),
            facets: AccessFacets {
                installed: true,
                discovered: true,
                user_reported: true,
                access_verified: false,
                execution_tested: false,
            },
            evidence: vec!["evidence-1".into()],
        }],
        repair_policy: None,
    }
}

fn approval(proposal: &ProfileSetupProposal) -> ProfileSetupApproval {
    ProfileSetupApproval {
        version: 1,
        id: "approval-1".into(),
        by: SetupApprover::Owner,
        at: now(),
        proposal: proposal.id.clone(),
        proposal_digest: proposal.digest().unwrap(),
        based_on_config_revision: proposal.based_on_config_revision.clone(),
        acknowledged_evidence_gap: false,
    }
}

fn store_fixture() -> (tempfile::TempDir, Store, Task) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let task = store
        .create("Fixture", None, Status::Backlog, "Bounded work")
        .unwrap();
    let task = store.get(&task.id).unwrap();
    (dir, store, task)
}

fn auto_selection(config: &Config, task: &Task) -> AutoProfileSelection {
    AutoProfileSelection {
        version: 1,
        id: "selection-1".into(),
        task: task.id.clone(),
        actor: SelectionActor::Proposer,
        selected_at: now(),
        role: ProfileRole::Implementation,
        profile: "omo".into(),
        profile_revision: "legacy".into(),
        spec_revision: plan_contract_revision(task).unwrap(),
        catalog_revision: config.profile_catalog_revision().unwrap(),
        reason: "No specialist fits this bounded work".into(),
        closest_alternative: None,
        distinction: None,
        review: None,
    }
}
