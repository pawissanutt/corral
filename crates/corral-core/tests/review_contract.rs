//! PRF-08 review requirement floor, contract versions and `auto` mapping validation.
use std::collections::BTreeSet;

use corral_core::*;

fn task(store: &Store, kind: Option<&str>, size: Option<TaskSize>, labels: &[&str]) -> Task {
    let created = store
        .create("Fixture", None, Status::Backlog, "Bounded work")
        .unwrap();
    let mut task = store.get(&created.id).unwrap();
    task.kind = kind.map(str::to_string);
    task.size = size;
    task.labels = labels.iter().map(|l| l.to_string()).collect();
    task
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    (dir, store)
}

fn floor(task: &Task, subject: ReviewSubjectKind) -> ReviewRequirement {
    review_requirement(&ReviewSignals::for_task(task, subject))
}

fn codes(requirement: &ReviewRequirement) -> Vec<ReviewReasonCode> {
    requirement.reasons.iter().map(|r| r.code).collect()
}

#[test]
fn plan_review_is_strong_regardless_of_kind_or_size() {
    let (_dir, store) = store();
    let mut small_docs_plan = task(&store, Some("docs"), Some(TaskSize::Xs), &[]);
    let plan = floor(&small_docs_plan, ReviewSubjectKind::Plan);
    assert_eq!(plan.contract.kind, ReviewContractKind::Plan);
    assert_eq!(plan.capability, ReviewCapability::Strong);
    assert!(plan.checks.contains(&ReviewCheck::RepositoryReferences));
    assert!(codes(&plan).contains(&ReviewReasonCode::SizeNotDowngrading));

    small_docs_plan.design = true;
    let design = floor(&small_docs_plan, ReviewSubjectKind::Plan);
    assert_eq!(design.contract.kind, ReviewContractKind::Design);
    assert!(design.checks.contains(&ReviewCheck::Alternatives));
}

#[test]
fn routine_implementation_uses_the_evidence_led_contract() {
    let (_dir, store) = store();
    let routine = floor(
        &task(&store, Some("logic"), Some(TaskSize::L), &[]),
        ReviewSubjectKind::Implementation,
    );
    assert_eq!(routine.contract.kind, ReviewContractKind::Implementation);
    assert_eq!(routine.capability, ReviewCapability::Routine);
    for check in [
        ReviewCheck::FullDiff,
        ReviewCheck::RelevantCode,
        ReviewCheck::CriterionEvidence,
        ReviewCheck::CandidateEvidence,
        ReviewCheck::ExhaustiveClassSweep,
    ] {
        assert!(routine.checks.contains(&check), "{check:?}");
    }
    assert_eq!(
        routine.contract.version,
        review_contract_version(routine.contract.kind)
    );
}

#[test]
fn small_or_prose_labelled_risky_change_cannot_downgrade() {
    let (_dir, store) = store();
    for labels in [&["concurrency"][..], &["permissions"], &["data-integrity"]] {
        let risky = floor(
            &task(&store, Some("docs"), Some(TaskSize::Xs), labels),
            ReviewSubjectKind::Implementation,
        );
        assert_eq!(
            risky.contract.kind,
            ReviewContractKind::ComplexImplementation
        );
        assert_eq!(risky.capability, ReviewCapability::Strong);
        assert!(risky.checks.contains(&ReviewCheck::Invariants));
        assert!(codes(&risky).contains(&ReviewReasonCode::RiskFlag));
        assert!(codes(&risky).contains(&ReviewReasonCode::SizeNotDowngrading));
    }
    let mut signals = ReviewSignals::for_task(
        &task(&store, Some("logic"), Some(TaskSize::Xs), &[]),
        ReviewSubjectKind::Implementation,
    );
    signals.risks.insert(ReviewRisk::RiskPath);
    assert_eq!(
        review_requirement(&signals).contract.kind,
        ReviewContractKind::ComplexImplementation
    );
}

#[test]
fn ui_prose_research_and_test_work_receive_their_checks() {
    let (_dir, store) = store();
    let ui = floor(
        &task(&store, Some("ui"), None, &[]),
        ReviewSubjectKind::Implementation,
    );
    assert_eq!(ui.contract.kind, ReviewContractKind::VisualImplementation);
    assert!(ui.checks.contains(&ReviewCheck::VisualEvidence));

    let risky_ui = floor(
        &task(&store, Some("ui"), None, &["authority"]),
        ReviewSubjectKind::Implementation,
    );
    assert_eq!(
        risky_ui.contract.kind,
        ReviewContractKind::ComplexImplementation
    );
    assert!(risky_ui.checks.contains(&ReviewCheck::VisualEvidence));

    let docs = floor(
        &task(&store, Some("docs"), None, &[]),
        ReviewSubjectKind::Implementation,
    );
    assert_eq!(docs.contract.kind, ReviewContractKind::Prose);
    assert_eq!(docs.capability, ReviewCapability::Routine);
    assert!(docs.checks.contains(&ReviewCheck::SourceProvenance));
    assert!(!docs.checks.contains(&ReviewCheck::RelevantCode));

    let research = floor(
        &task(&store, Some("research"), None, &[]),
        ReviewSubjectKind::Implementation,
    );
    assert_eq!(research.contract.kind, ReviewContractKind::Prose);
    assert_eq!(research.capability, ReviewCapability::Strong);

    for (kind, labels) in [(Some("test"), &[][..]), (Some("logic"), &["flaky"][..])] {
        let test = floor(
            &task(&store, kind, None, labels),
            ReviewSubjectKind::Implementation,
        );
        assert_eq!(test.contract.kind, ReviewContractKind::TestOnly);
        assert!(test.checks.contains(&ReviewCheck::DeterministicFix));
        assert!(test.checks.contains(&ReviewCheck::RegressionCanFail));
    }
}

#[test]
fn refactor_and_hotfix_add_scoped_checks() {
    let (_dir, store) = store();
    let refactor = floor(
        &task(&store, Some("refactor"), None, &[]),
        ReviewSubjectKind::Implementation,
    );
    assert_eq!(refactor.contract.kind, ReviewContractKind::Implementation);
    assert!(refactor.checks.contains(&ReviewCheck::BehaviorUnchanged));
    let hotfix = floor(
        &task(&store, Some("logic"), None, &["hotfix"]),
        ReviewSubjectKind::Implementation,
    );
    assert!(hotfix.checks.contains(&ReviewCheck::MinimalScope));
    assert!(codes(&hotfix).contains(&ReviewReasonCode::Hotfix));
}

#[test]
fn stronger_request_raises_capability_and_round_three_scopes_review() {
    let (_dir, store) = store();
    let mut signals = ReviewSignals::for_task(
        &task(&store, Some("logic"), None, &[]),
        ReviewSubjectKind::Implementation,
    );
    signals.stronger_requested = true;
    let raised = review_requirement(&signals);
    assert_eq!(raised.capability, ReviewCapability::Strong);
    assert!(codes(&raised).contains(&ReviewReasonCode::StrongerRequested));
    assert_eq!(review_scope(1), ReviewScope::Full);
    assert_eq!(review_scope(2), ReviewScope::Full);
    assert_eq!(
        review_scope(ROUND_SCOPED_FROM),
        ReviewScope::PriorFindingsAndFixes
    );
}

fn reviewer_profile(roles: &[ProfileRole]) -> Profile {
    Profile {
        metadata: Some(ProfileMetadata {
            display_name: "Reviewer".into(),
            purpose: "Review".into(),
            guidance: SelectionGuidance::default(),
            instructions: "Check migrations".into(),
            eligibility: ProfileEligibility {
                enabled: true,
                automatic: true,
                roles: roles.iter().copied().collect::<BTreeSet<_>>(),
                projects: BTreeSet::new(),
            },
            revision: "r1".into(),
            evidence: None,
            candidates: vec![],
            fallback: None,
        }),
        harness: "omo".into(),
        model: None,
        vendor: None,
        effort: None,
        args: vec![],
        escalate_to: None,
    }
}

#[test]
fn auto_policy_is_an_explicit_keyword_with_validated_mappings() {
    assert_eq!(Reviewer::parse("auto"), Reviewer::Auto);
    assert_eq!(Reviewer::parse("pm"), Reviewer::Pm);
    assert_eq!(Reviewer::parse("cross-vendor"), Reviewer::CrossVendor);
    assert_eq!(Reviewer::parse("none"), Reviewer::None);
    assert_eq!(
        Reviewer::parse("custom.db"),
        Reviewer::Profile("custom.db".into())
    );

    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    config.review.reviewer = Some("auto".into());
    let problems = config.review_problems();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("routing.review_profiles.plan")),
        "{problems:?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.contains("routing.review_profiles.implementation")),
        "{problems:?}"
    );

    use ProfileRole::{ImplementationReview, PlanReview};
    config
        .profiles
        .insert("routine".into(), reviewer_profile(&[ImplementationReview]));
    config.profiles.insert(
        "custom.database".into(),
        reviewer_profile(&[PlanReview, ImplementationReview]),
    );
    config
        .routing
        .review_profiles
        .insert(ReviewContractKind::Plan, "routine".into());
    config
        .routing
        .review_profiles
        .insert(ReviewContractKind::Implementation, "routine".into());
    config
        .routing
        .review_profiles
        .insert(ReviewContractKind::ComplexImplementation, "routine".into());
    let problems = config.review_problems();
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("routing.review_profiles.plan")),
        "{problems:?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("routing.review_profiles.complex_implementation")),
        "{problems:?}"
    );

    config
        .routing
        .review_profiles
        .insert(ReviewContractKind::Plan, "custom.database".into());
    config.routing.review_profiles.insert(
        ReviewContractKind::ComplexImplementation,
        "custom.database".into(),
    );
    assert!(
        config.review_problems().is_empty(),
        "{:?}",
        config.review_problems()
    );
}

#[test]
fn mapping_fallback_never_weakens_a_strong_contract() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    config
        .routing
        .review_profiles
        .insert(ReviewContractKind::Plan, "strong".into());
    config
        .routing
        .review_profiles
        .insert(ReviewContractKind::Implementation, "routine".into());
    assert_eq!(
        config.review_profile_for(ReviewContractKind::ComplexImplementation),
        Some((ReviewContractKind::Plan, "strong"))
    );
    assert_eq!(
        config.review_profile_for(ReviewContractKind::VisualImplementation),
        Some((ReviewContractKind::Implementation, "routine"))
    );
    config
        .routing
        .review_profiles
        .remove(&ReviewContractKind::Plan);
    assert_eq!(
        config.review_profile_for(ReviewContractKind::ComplexImplementation),
        None
    );
}

#[test]
fn unset_risk_paths_keep_existing_policy_identity() {
    let policy = ReviewPolicy::default();
    let json = serde_json::to_value(&policy).unwrap();
    assert!(json.get("risk_paths").is_none(), "{json}");
    let mut config = Config::default_for(std::path::Path::new("."));
    config.review.risk_paths = Some(vec!["/abs".into()]);
    assert!(
        config
            .review_problems()
            .iter()
            .any(|p| p.contains("risk_paths"))
    );
}

#[test]
fn auto_plan_reviewer_uses_the_plan_mapping() {
    let (_dir, mut store) = store();
    store.config.review.reviewer = Some("auto".into());
    store
        .config
        .routing
        .review_profiles
        .insert(ReviewContractKind::Plan, "custom.database".into());
    store.config.profiles.insert(
        "custom.database".into(),
        reviewer_profile(&[ProfileRole::PlanReview, ProfileRole::ImplementationReview]),
    );
    let task = task(&store, Some("logic"), None, &[]);
    assert_eq!(
        plan_policy::expected_reviewer_for(&store.config, &task, None, None).unwrap(),
        "custom.database"
    );
}
