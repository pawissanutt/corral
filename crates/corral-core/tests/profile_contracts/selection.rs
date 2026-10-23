use super::*;

fn subject(task: &Task) -> ProfileReviewSubject {
    ProfileReviewSubject::Plan {
        task: task.id.clone(),
        plan: "P1".into(),
        spec_revision: plan_contract_revision(task).unwrap(),
        attempt: "review-1".into(),
    }
}

fn review_selection(config: &Config, task: &Task) -> ProfileReviewSelection {
    let policy = config.task_review_policy(task);
    ProfileReviewSelection {
        version: 1,
        profile: "custom.database".into(),
        profile_revision: "profile-r1".into(),
        contract: ReviewContract {
            kind: ReviewContractKind::Plan,
            version: corral_core::review_contract_version(ReviewContractKind::Plan),
        },
        subject: subject(task),
        policy: ReviewerPolicyRecord {
            source: PolicySource::Global,
            selector: reviewer_selector(&policy.reviewer),
            require_cross_family: policy.cross_family_required(),
            policy_digest: corral_core::profile_selection::reviewer_policy_digest(&policy).unwrap(),
        },
        reason: "Inspect the transaction design".into(),
    }
}

#[test]
fn review_journal_requires_exact_context_and_retains_task_pm_override() {
    // Given a task-level PM reviewer override above a named workspace reviewer.
    let (_dir, mut store, mut task) = store_fixture();
    store
        .config
        .profiles
        .insert("custom.database".into(), custom_profile());
    store.config.review.reviewer = Some("custom.database".into());
    store.save_config().unwrap();
    task.reviewer = Some("pm".into());
    store.save(&task).unwrap();
    let task = store.get(&task.id).unwrap();
    let mut review = review_selection(&store.config, &task);
    review.profile = "omo".into();
    review.profile_revision = "legacy".into();
    review.policy.source = PolicySource::Task;
    let expected = subject(&task);
    let context = ReviewSelectionContext {
        task: &task,
        subject: &expected,
        author_vendor: Some("openai"),
    };
    let mut receipt = auto_selection(&store.config, &task);
    receipt.role = ProfileRole::PlanReview;
    receipt.review = Some(review);
    // When storing without the actual review context, then with it.
    let missing_context = store.save_auto_profile_selection(&receipt, None);
    let saved = store.save_auto_profile_selection(&receipt, Some(&context));
    // Then the task override is retained as PM policy, not relabelled independent review.
    assert!(missing_context.is_err());
    assert!(saved.is_ok());
    let records = store.auto_profile_selections().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].review.as_ref().unwrap().policy.source,
        PolicySource::Task
    );
    assert_eq!(records[0].review.as_ref().unwrap().policy.selector, "pm");
}

#[test]
fn auto_receipt_round_trip_preserves_actor_without_creating_manual_pin() {
    // Given a proposer receipt, distinct from Task.profile.
    let (_dir, store, task) = store_fixture();
    let receipt = auto_selection(&store.config, &task);
    // When the structured receipt round-trips.
    let decoded: AutoProfileSelection =
        serde_json::from_str(&serde_json::to_string(&receipt).unwrap()).unwrap();
    // Then selection remains an Auto choice while the task stays unpinned.
    assert!(decoded.validate(&store.config, &task).is_ok());
    assert_eq!(decoded.actor, SelectionActor::Proposer);
    assert_eq!(task.profile, None);
}

#[test]
fn auto_selection_rejects_stale_task_catalog_role_and_profile() {
    // Given an initially valid selection of a custom profile.
    let (_dir, store, task) = store_fixture();
    let mut config = store.config.clone();
    config
        .profiles
        .insert("custom.database".into(), custom_profile());
    for case in [
        "task", "catalog", "role", "profile", "revision", "version", "review",
    ] {
        let mut receipt = auto_selection(&config, &task);
        receipt.profile = "custom.database".into();
        receipt.profile_revision = "profile-r1".into();
        let mut changed_task = task.clone();
        let mut changed_config = config.clone();
        match case {
            "task" => changed_task.body.push_str("Changed scope"),
            "catalog" => changed_config
                .profiles
                .get_mut("custom.database")
                .unwrap()
                .metadata
                .as_mut()
                .unwrap()
                .guidance
                .use_when
                .clear(),
            "role" => receipt.role = ProfileRole::Pm,
            "profile" => receipt.profile = "missing".into(),
            "revision" => receipt.profile_revision = "old".into(),
            "version" => receipt.version = 2,
            "review" => receipt.role = ProfileRole::PlanReview,
            _ => unreachable!(),
        }
        // When checking a future dispatch's receipt.
        let result = receipt.validate(&changed_config, &changed_task);
        // Then stale or ineligible choices cannot be reused.
        assert!(result.is_err(), "{case}");
    }
}

#[test]
fn review_identity_keeps_contract_profile_and_plan_attempt_separate() {
    // Given a named reviewer with independent plan-review identity.
    let (_dir, store, task) = store_fixture();
    let mut config = store.config.clone();
    config
        .profiles
        .insert("custom.database".into(), custom_profile());
    config.review.reviewer = Some("custom.database".into());
    let selection = review_selection(&config, &task);
    let expected = subject(&task);
    let context = ReviewSelectionContext {
        task: &task,
        subject: &expected,
        author_vendor: Some("openai"),
    };
    // When the selection round-trips and validates against the exact plan/attempt.
    let decoded: ProfileReviewSelection =
        serde_json::from_str(&serde_json::to_string(&selection).unwrap()).unwrap();
    // Then a custom reviewer profile retains the separate plan contract.
    assert!(decoded.validate(&config, &context).is_ok());
    assert_eq!(decoded.subject.role(), ProfileRole::PlanReview);
    assert_eq!(decoded.contract.kind, ReviewContractKind::Plan);
    let mut wrong = decoded;
    wrong.contract.kind = ReviewContractKind::Implementation;
    assert!(wrong.validate(&config, &context).is_err());
}

#[test]
fn review_rejects_rebound_plan_attempt_and_forged_policy_provenance() {
    // Given a valid plan reviewer.
    let (_dir, store, task) = store_fixture();
    let mut config = store.config.clone();
    config
        .profiles
        .insert("custom.database".into(), custom_profile());
    config.review.reviewer = Some("custom.database".into());
    let expected = subject(&task);
    let context = ReviewSelectionContext {
        task: &task,
        subject: &expected,
        author_vendor: Some("openai"),
    };
    for case in ["attempt", "source", "selector", "policy", "profile", "none"] {
        let mut selection = review_selection(&config, &task);
        let mut changed = config.clone();
        match case {
            "attempt" => match &mut selection.subject {
                ProfileReviewSubject::Plan { attempt, .. } => *attempt = "another-attempt".into(),
                ProfileReviewSubject::Implementation { .. } => unreachable!(),
            },
            "source" => selection.policy.source = PolicySource::Default,
            "selector" => selection.policy.selector = "pm".into(),
            "policy" => changed.review.require_cross_family = Some(true),
            "profile" => selection.profile = "omo".into(),
            "none" => changed.review.reviewer = Some("none".into()),
            _ => unreachable!(),
        }
        // When checking actual review identity and effective policy.
        let result = selection.validate(&changed, &context);
        // Then neither another attempt nor an explicit policy can be overwritten.
        assert!(result.is_err(), "{case}");
    }
}

#[test]
fn implementation_identity_and_cross_family_requirement_survive_profile_selection() {
    // Given an implementation candidate reviewed under a distinct review contract.
    let (_dir, store, task) = store_fixture();
    let mut config = store.config.clone();
    config
        .profiles
        .insert("custom.database".into(), custom_profile());
    config.review.reviewer = Some("custom.database".into());
    config.review.require_cross_family = Some(true);
    let candidate = CandidateIdentity {
        version: 2,
        task: task.id.clone(),
        run: "R1".into(),
        attempt: "A1".into(),
        head: "head".into(),
        tree: "tree".into(),
        integration_base: "base".into(),
        dispatch_base: "dispatch".into(),
        spec_digest: "spec".into(),
        policy_digest: "policy".into(),
        inputs_digest: "inputs".into(),
        environment: vec![],
        gate_tier: None,
    };
    let expected = ProfileReviewSubject::Implementation {
        candidate: Box::new(candidate),
    };
    let mut selection = review_selection(&config, &task);
    selection.subject = expected.clone();
    selection.contract.kind = ReviewContractKind::ComplexImplementation;
    selection.contract.version =
        corral_core::review_contract_version(ReviewContractKind::ComplexImplementation);
    // When the reviewer is from the same family, then another family.
    let same = selection.validate(
        &config,
        &ReviewSelectionContext {
            task: &task,
            subject: &expected,
            author_vendor: Some("openai"),
        },
    );
    let other = selection.validate(
        &config,
        &ReviewSelectionContext {
            task: &task,
            subject: &expected,
            author_vendor: Some("anthropic"),
        },
    );
    // Then explicit cross-family policy still constrains the actual reviewer.
    assert!(same.is_err());
    assert!(other.is_ok());
    assert_eq!(selection.subject.role(), ProfileRole::ImplementationReview);
}
