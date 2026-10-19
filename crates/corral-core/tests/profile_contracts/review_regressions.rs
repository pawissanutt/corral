use super::*;

fn repair_event(id: &str, kind: RepairAttemptKind) -> RepairEvent {
    let (profile, findings) = match &kind {
        RepairAttemptKind::InitialImplementation => ("omo", vec![]),
        RepairAttemptKind::StrongerProfileRepair => ("claude", vec![FindingId("F1".into())]),
        RepairAttemptKind::CurrentProfileRepair
        | RepairAttemptKind::AvailabilityRecovery { .. }
        | RepairAttemptKind::PmDiagnosis => ("omo", vec![FindingId("F1".into())]),
    };
    RepairEvent::Started {
        receipt: RepairStartReceipt {
            attempt_id: RepairAttemptId(id.into()),
            profile: profile.into(),
            kind,
            findings,
        },
    }
}

fn history_at(stage: u32) -> RepairHistory {
    let mut history = RepairHistory::new(
        RepairPolicy::approved_starter_policy(),
        "omo",
        Some("claude".into()),
    )
    .unwrap();
    history
        .apply(repair_event(
            "initial",
            RepairAttemptKind::InitialImplementation,
        ))
        .unwrap();
    let identity = FailureIdentity {
        criterion_or_test: "C1".into(),
        affected_contract_or_location: "store::commit".into(),
        failure_code: "partial_write".into(),
    };
    history
        .apply(RepairEvent::Finding {
            finding: RepairFinding {
                observation_id: "O1".into(),
                finding_id: FindingId("F1".into()),
                fingerprint: identity.fingerprint(),
                identity,
                observed_failure: "Partial write".into(),
                matching: FindingMatch::New,
                attempted_fixes: vec![],
                resolution: FindingResolution::Unresolved,
            },
        })
        .unwrap();
    for (index, id, kind) in [
        (1, "c1", RepairAttemptKind::CurrentProfileRepair),
        (2, "c2", RepairAttemptKind::CurrentProfileRepair),
        (3, "s1", RepairAttemptKind::StrongerProfileRepair),
    ] {
        if stage >= index {
            history.apply(repair_event(id, kind)).unwrap();
        }
    }
    history
}

#[test]
fn reviewer_probe_old_recovery_cannot_return_from_stronger_stage() {
    // Given later coding attempts, including advancement to the stronger stage.
    for (stage, interrupted) in [(1, "initial"), (2, "c1"), (3, "c2"), (3, "c1")] {
        let mut history = history_at(stage);
        let before = history.clone();
        let mut event = repair_event(
            "stale-recovery",
            RepairAttemptKind::AvailabilityRecovery {
                interrupted_attempt: RepairAttemptId(interrupted.into()),
            },
        );
        if interrupted == "initial"
            && let RepairEvent::Started { receipt } = &mut event
        {
            receipt.findings.clear();
        }
        // When recovering a superseded attempt under a new receipt ID.
        let result = history.apply(event);
        // Then neither history nor counters change.
        assert!(result.is_err(), "stage {stage}, interrupted {interrupted}");
        assert_eq!(history, before);
    }
}

#[test]
fn latest_coding_attempt_recovery_is_valid_at_each_stage_boundary() {
    // Given the latest coding attempt, including exhausted-stage boundaries.
    for (stage, id, profile) in [
        (0, "initial", "omo"),
        (1, "c1", "omo"),
        (2, "c2", "omo"),
        (3, "s1", "claude"),
    ] {
        let mut history = history_at(stage);
        let repairs = history.counters().unwrap().coding_repairs;
        let mut event = repair_event(
            "latest",
            RepairAttemptKind::AvailabilityRecovery {
                interrupted_attempt: RepairAttemptId(id.into()),
            },
        );
        if let RepairEvent::Started { receipt } = &mut event {
            receipt.profile = profile.into();
            if stage == 0 {
                receipt.findings.clear();
            }
        }
        // When recovering the current attempt without starting a new coding repair.
        let result = history.apply(event);
        // Then recovery remains valid and consumes no new coding allowance.
        assert_eq!(result.unwrap(), ReceiptApplication::Recorded);
        assert_eq!(history.counters().unwrap().coding_repairs, repairs);
    }
}

#[test]
fn latest_recovery_and_identical_redelivery_survive_store_restart() {
    // Given recovery of the latest current-stage attempt persisted in an isolated store.
    let (dir, store, task) = store_fixture();
    let mut history = history_at(1);
    let recovery = repair_event(
        "recover-c1",
        RepairAttemptKind::AvailabilityRecovery {
            interrupted_attempt: RepairAttemptId("c1".into()),
        },
    );
    assert_eq!(
        history.apply(recovery.clone()).unwrap(),
        ReceiptApplication::Recorded
    );
    store
        .save_profile_repair_history(&task.id, &history)
        .unwrap();
    let reopened = Store::open(dir.path()).unwrap();
    let mut history = reopened
        .profile_repair_histories()
        .unwrap()
        .remove(&task.id)
        .unwrap();
    history
        .apply(repair_event("c2", RepairAttemptKind::CurrentProfileRepair))
        .unwrap();
    history
        .apply(repair_event("s1", RepairAttemptKind::StrongerProfileRepair))
        .unwrap();
    // When redelivering the same recovery after newer coding attempts and restart.
    let result = history.apply(recovery);
    // Then the historical duplicate remains idempotent, but no new old-stage recovery is admitted.
    assert_eq!(result.unwrap(), ReceiptApplication::Duplicate);
    assert_eq!(history.counters().unwrap().availability_recoveries, 1);
    let mut latest = repair_event(
        "recover-s1",
        RepairAttemptKind::AvailabilityRecovery {
            interrupted_attempt: RepairAttemptId("s1".into()),
        },
    );
    if let RepairEvent::Started { receipt } = &mut latest {
        receipt.profile = "claude".into();
    }
    assert_eq!(history.apply(latest).unwrap(), ReceiptApplication::Recorded);
    reopened
        .save_profile_repair_history(&task.id, &history)
        .unwrap();
    assert_eq!(
        Store::open(dir.path())
            .unwrap()
            .profile_repair_histories()
            .unwrap()[&task.id],
        history
    );
}

#[test]
fn restart_and_store_loading_reject_serialized_superseded_recovery() {
    // Given a journal with a newly appended recovery of an old current-stage repair.
    let (_dir, store, task) = store_fixture();
    let history = history_at(3);
    store
        .save_profile_repair_history(&task.id, &history)
        .unwrap();
    let mut value = serde_json::to_value(history).unwrap();
    value["events"].as_array_mut().unwrap().push(
        serde_json::to_value(repair_event(
            "stale",
            RepairAttemptKind::AvailabilityRecovery {
                interrupted_attempt: RepairAttemptId("c1".into()),
            },
        ))
        .unwrap(),
    );
    std::fs::write(
        store.state_dir().join("profile-repairs.json"),
        serde_json::to_vec(
            &serde_json::json!({"version": 1, "histories": {task.id.clone(): value.clone()}}),
        )
        .unwrap(),
    )
    .unwrap();
    // When replaying that sequence in memory and through the store boundary.
    let restored = serde_json::from_value::<RepairHistory>(value);
    let loaded = store.profile_repair_histories();
    // Then both boundaries reject the same invalid transition.
    assert!(restored.is_err());
    assert!(loaded.is_err());
}

fn plan_selection(
    config: &Config,
    task: &Task,
    planner: Option<plan_policy::PlannerProfile<'_>>,
) -> ProfileReviewSelection {
    let expected = plan_policy::expected_reviewer_for(config, task, planner, None).unwrap();
    let policy = config.task_review_policy(task);
    let profile = if expected == "pm" {
        config.pm.agent.clone()
    } else {
        expected
    };
    let revision = config
        .profiles
        .get(&profile)
        .and_then(|p| p.metadata.as_ref())
        .map(|m| m.revision.clone())
        .unwrap_or_else(|| "legacy".into());
    ProfileReviewSelection {
        version: 1,
        profile,
        profile_revision: revision,
        contract: ReviewContract {
            kind: ReviewContractKind::Plan,
            version: 1,
        },
        subject: ProfileReviewSubject::Plan {
            task: task.id.clone(),
            plan: "P1".into(),
            spec_revision: plan_contract_revision(task).unwrap(),
            attempt: "review-1".into(),
        },
        policy: ReviewerPolicyRecord {
            source: PolicySource::Default,
            selector: reviewer_selector(&policy.reviewer),
            require_cross_family: policy.cross_family_required(),
            policy_digest: reviewer_policy_digest(&policy).unwrap(),
        },
        reason: "Preserve planner-aware review policy".into(),
    }
}

#[test]
fn reviewer_probe_plan_none_preserves_existing_independent_review_policy() {
    // Given the default None policy and a prospective planner before its run exists.
    let (_dir, store, task) = store_fixture();
    let mut config = store.config.clone();
    config
        .profiles
        .insert("custom.database".into(), custom_profile());
    let planner = plan_policy::PlannerProfile {
        profile: "custom.database",
        model: Some("openai/exact-fixture-v1"),
        vendor: Some("openai"),
    };
    let selection = plan_selection(&config, &task, Some(planner));
    plan_policy::independent_reviewer_for(&config, &task, Some(planner), &selection.profile)
        .unwrap();
    let context = ReviewSelectionContext {
        task: &task,
        subject: &selection.subject,
        author_vendor: Some("openai"),
    };
    // When preview supplies the same actual prospective planner to receipt validation.
    let result = selection.validate_with_planner(&config, &context, Some(planner));
    // Then the existing default plan reviewer is permitted without changing owner policy.
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(config.task_review_policy(&task).reviewer, Reviewer::None);
    // No planner means owner review, so this independent-agent receipt is not accepted.
    assert!(selection.validate(&config, &context).is_err());
}

#[test]
fn recorded_planner_default_review_uses_existing_policy_through_store_restart() {
    // Given an actual recorded planner-authored plan under the default None policy.
    let (dir, mut store, mut task) = store_fixture();
    store
        .config
        .profiles
        .insert("custom.database".into(), custom_profile());
    store.save_config().unwrap();
    let revision = plan_contract_revision(&task).unwrap();
    task.plan_runs.push(PlanRun {
        execution: None,
        route: None,
        delivery: corral_core::inbox::DeliveryMode::Legacy,
        inbox: Default::default(),
        id: "D1".into(),
        profile: "custom.database".into(),
        harness: "omo".into(),
        model: Some("openai/exact-fixture-v1".into()),
        vendor: Some("openai".into()),
        effort: Some("high".into()),
        spec_revision: revision.clone(),
        started: now(),
        ended: Some(now()),
        herdr: None,
        output_dir: "plans/D1".into(),
        submitted: Some("P1".into()),
        rounds: 0,
        state: PlanRunState::Submitted,
        pending_reply: None,
    });
    task.plans.push(PlanRecord {
        id: "P1".into(),
        run: Some("D1".into()),
        approval: None,
        spec_revision: revision,
        reviews: vec![],
        artifact: Artifact {
            tier_override: None,
            compressed_bytes: None,
            purged_at: None,
            run: "D1".into(),
            kind: ArtifactKind::Plan,
            title: "Plan".into(),
            path: "plans/P1.md".into(),
            source: "fixture-plan.md".into(),
            sha256: "plan-digest".into(),
            bytes: 1,
            added: now(),
        },
        document: PlanDocument {
            delivery_target: None,
            goal: "Bounded work".into(),
            non_goals: vec![],
            decisions: vec![],
            affected_paths: vec!["core".into()],
            contracts: vec![],
            dependencies: vec![],
            acceptance: vec![PlanAcceptance {
                criterion: None,
                scenario: "Review the plan".into(),
                expected: "Validated receipt".into(),
            }],
            verification: vec!["cargo test".into()],
            owner_decisions: vec![],
        },
    });
    store.save(&task).unwrap();
    let task = store.get(&task.id).unwrap();
    let planner = plan_policy::planner(&task)
        .unwrap()
        .map(plan_policy::PlannerProfile::from);
    let selection = plan_selection(&store.config, &task, planner);
    let context = ReviewSelectionContext {
        task: &task,
        subject: &selection.subject,
        author_vendor: Some("openai"),
    };
    assert!(selection.validate(&store.config, &context).is_ok());
    assert!(
        selection
            .validate_with_planner(&store.config, &context, None)
            .is_err()
    );
    let mut receipt = auto_selection(&store.config, &task);
    receipt.role = ProfileRole::PlanReview;
    receipt.profile = selection.profile.clone();
    receipt.profile_revision = selection.profile_revision.clone();
    receipt.review = Some(selection);
    let expected = receipt.review.as_ref().unwrap().subject.clone();
    // When this default plan-review receipt is saved and the store reopened.
    store
        .save_auto_profile_selection(
            &receipt,
            Some(&ReviewSelectionContext {
                task: &task,
                subject: &expected,
                author_vendor: Some("openai"),
            }),
        )
        .unwrap();
    let reopened = Store::open(dir.path()).unwrap();
    // Then the same policy-valid plan receipt survives without relabelling None as disabled.
    assert_eq!(reopened.auto_profile_selections().unwrap(), vec![receipt]);
}

#[test]
fn implementation_none_still_disables_independent_reviewer_selection() {
    // Given an implementation candidate under the default None reviewer policy.
    let (_dir, store, task) = store_fixture();
    let mut selection = plan_selection(
        &store.config,
        &task,
        Some(plan_policy::PlannerProfile {
            profile: "omo",
            model: None,
            vendor: None,
        }),
    );
    selection.subject = ProfileReviewSubject::Implementation {
        candidate: Box::new(CandidateIdentity {
            version: 2,
            task: task.id.clone(),
            run: "R1".into(),
            attempt: "A1".into(),
            head: "head".into(),
            tree: "tree".into(),
            integration_base: "base".into(),
            dispatch_base: "base".into(),
            spec_digest: "spec".into(),
            policy_digest: "policy".into(),
            inputs_digest: "inputs".into(),
            environment: vec![],
        }),
    };
    selection.contract.kind = ReviewContractKind::Implementation;
    // When applying the implementation role's existing None semantics.
    let result = selection.validate(
        &store.config,
        &ReviewSelectionContext {
            task: &task,
            subject: &selection.subject,
            author_vendor: None,
        },
    );
    // Then independent review remains disabled for implementation.
    assert!(result.is_err());
}

#[test]
fn planner_review_keeps_cross_family_and_exact_planner_context_checks() {
    // Given a default plan reviewer policy with an explicit cross-family requirement.
    let (_dir, store, task) = store_fixture();
    let mut config = store.config.clone();
    let mut author = custom_profile();
    author.vendor = Some("openai".into());
    config.profiles.insert("author".into(), author);
    let mut reviewer = custom_profile();
    reviewer.vendor = Some("anthropic".into());
    reviewer.model = Some("anthropic/review-v1".into());
    let candidate = &mut reviewer.metadata.as_mut().unwrap().candidates[0];
    candidate.provider = "anthropic".into();
    candidate.model = "anthropic/review-v1".into();
    config.profiles.insert("independent".into(), reviewer);
    config.review.require_cross_family = Some(true);
    let planner = plan_policy::PlannerProfile {
        profile: "author",
        model: Some("openai/exact-fixture-v1"),
        vendor: Some("openai"),
    };
    let selection = plan_selection(&config, &task, Some(planner));
    let context = ReviewSelectionContext {
        task: &task,
        subject: &selection.subject,
        author_vendor: Some("openai"),
    };
    // When the selected opposite-family reviewer is validated, versus a changed planner vendor.
    let accepted = selection.validate_with_planner(&config, &context, Some(planner));
    let rejected = selection.validate_with_planner(
        &config,
        &context,
        Some(plan_policy::PlannerProfile {
            profile: "author",
            model: Some("openai/exact-fixture-v1"),
            vendor: Some("anthropic"),
        }),
    );
    // Then independence is retained rather than waived for None policy.
    assert!(accepted.is_ok(), "{accepted:?}");
    assert!(rejected.is_err());
}

#[test]
fn reviewer_probe_history_rejects_future_nested_evidence_version() {
    // Given version-one setup history and independently corrupted nested records.
    for case in ["version", "identity", "source", "evidence_ref"] {
        let (_dir, store, _task) = store_fixture();
        store
            .save_profile_setup(&ProfileSetupRecord {
                revision: 1,
                state: ProfileSetupState::Gathering,
                ..Default::default()
            })
            .unwrap();
        let proposed = ProfileSetupRecord {
            revision: 2,
            state: ProfileSetupState::Proposed,
            proposal: Some(proposal()),
            ..Default::default()
        };
        store.save_profile_setup(&proposed).unwrap();
        let mut value = serde_json::to_value(proposed).unwrap();
        match case {
            "version" => value["proposal"]["evidence"]["evidence-1"]["version"] = 99.into(),
            "identity" => value["proposal"]["evidence"]["evidence-1"]["id"] = "other".into(),
            "source" => value["proposal"]["evidence"]["evidence-1"]["source"]["method"] = "".into(),
            "evidence_ref" => {
                value["proposal"]["recommendations"]["custom.database"]["evidence"] =
                    serde_json::json!(["missing"])
            }
            _ => unreachable!(),
        }
        std::fs::write(store.state_dir().join("profile-setup.json"), serde_json::to_vec(
            &serde_json::json!({"version": 1, "records": [
                ProfileSetupRecord { revision: 1, state: ProfileSetupState::Gathering, ..Default::default() }, value
            ]})).unwrap()).unwrap();
        // When history is loaded independently of today's configuration.
        let result = store.profile_setup_history();
        // Then unsupported nested versions and broken local integrity are rejected.
        assert!(result.is_err(), "{case}");
    }
}

#[test]
fn stale_historical_mapping_remains_inspectable_after_harness_settings_change() {
    // Given a historical proposal using high effort, valid under its original catalog.
    let (dir, mut store, _task) = store_fixture();
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
        proposal: Some(proposal()),
        ..Default::default()
    };
    store.save_profile_setup(&record).unwrap();
    store.config.harnesses.get_mut("omo").unwrap().efforts = vec!["low".into()];
    store.save_config().unwrap();
    // When the live configuration no longer supports that historical mapping.
    let reopened = Store::open(dir.path()).unwrap();
    // Then history remains inspectable but cannot be reapplied as a currently valid mapping.
    assert!(
        record
            .proposal
            .as_ref()
            .unwrap()
            .validate(&reopened.config)
            .is_err()
    );
    assert_eq!(
        reopened.profile_setup_history().unwrap().last(),
        Some(&record)
    );
}
