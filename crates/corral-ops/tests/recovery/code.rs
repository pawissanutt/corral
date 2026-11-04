use super::*;

#[test]
fn missing_receipts_are_hard_and_completed_failed_checks_remain_failed() {
    if isolated("code::missing_receipts_are_hard_and_completed_failed_checks_remain_failed") {
        return;
    }
    let mut f = crate::submission_paths_tests::fixture::Fx::new();
    f.ctx.store.config.review.gate = Some(vec!["false".into()]);
    f.ctx.store.config.harnesses.insert(
        "fixture".into(),
        serde_json::from_value(
            serde_json::json!({"command":"true {model_args}","model_flag":"--model"}),
        )
        .unwrap(),
    );
    for (name, model) in [
        ("fake", "author-model"),
        ("rev", "configured-model"),
        ("astra", "recovery-model"),
    ] {
        f.ctx.store.config.profiles.insert(
            name.into(),
            serde_json::from_value(
                serde_json::json!({"harness":"fixture","model":model,"vendor":"openai"}),
            )
            .unwrap(),
        );
    }
    f.ctx.store.save_config().unwrap();
    let id = f.task("Code acceptance", Status::Ready).id;
    f.start(&id);
    f.done(&id, "candidate.txt");
    let input = RecoveryInput {
        action: RecoveryAction::AcceptCode {
            merge: RecoveryMerge::Keep,
        },
        exceptions: BTreeSet::from([RecoveryCheck::GateOutcome, RecoveryCheck::ReviewOutcome]),
        notes: String::new(),
    };
    let missing = recover_preview(&f.ctx, &id, &input).unwrap();
    assert!(!missing.remaining_blockers.is_empty());
    assert!(
        missing
            .checks
            .iter()
            .all(|c| c.disposition == CheckDisposition::NotApplicable)
    );
    assert!(recover_apply(&f.ctx, &missing, &input, Some(confirmation(&missing))).is_err());
    f.ctx
        .store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .review
        .require_cross_family = Some(true);
    f.ctx.store.save_config().unwrap();
    crate::gate_start(&f.ctx, &id).unwrap();
    let gate = crate::gate_collect(&f.ctx, &id).unwrap().unwrap();
    assert_eq!(gate.result, GateResult::Fail);
    let review_input = RecoveryInput {
        action: RecoveryAction::UseReviewer {
            stage: RecoveryReviewStage::CodeReview,
            reviewer: policy::reviewer(&f.ctx.store.config, "astra").unwrap(),
        },
        exceptions: BTreeSet::from([RecoveryCheck::ReviewerFamilyPolicy]),
        notes: String::new(),
    };
    let review_preview = recover_preview(&f.ctx, &id, &review_input).unwrap();
    assert!(
        review_preview.remaining_blockers.is_empty(),
        "{:?}",
        review_preview.remaining_blockers
    );
    let launched = recover_apply(
        &f.ctx,
        &review_preview,
        &review_input,
        Some(confirmation(&review_preview)),
    )
    .unwrap();
    assert_eq!(
        state(&launched).unwrap(),
        RecoveryState::Applied,
        "{:?}",
        launched.outcomes
    );
    let current = f.ctx.store.get(&id).unwrap();
    let callback =
        crate::review_callback_token(current.current_run().unwrap().review.as_ref().unwrap())
            .unwrap();
    crate::verdict_bound(
        &f.ctx,
        &id,
        Verdict::Escalate,
        "Genuine separate reviewer: owner intervention needed",
        Some(&callback),
    )
    .unwrap();
    let mut input = input;
    input.exceptions.insert(RecoveryCheck::ReviewerFamilyPolicy);
    let preview = recover_preview(&f.ctx, &id, &input).unwrap();
    assert!(
        preview.remaining_blockers.is_empty(),
        "{:?}",
        preview.remaining_blockers
    );
    let record = recover_apply(&f.ctx, &preview, &input, Some(confirmation(&preview))).unwrap();
    assert_eq!(
        state(&record).unwrap(),
        RecoveryState::Applied,
        "{:?}",
        record.outcomes
    );
    let task = f.ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::Done);
    let events = f.ctx.store.events().unwrap();
    let approved = events.iter().rev().find(|e| e.kind == "approved").unwrap();
    assert_eq!(approved.detail["acceptance_mode"], "owner_recovery");
    assert_eq!(approved.detail["policy_satisfied"], false);
    assert_eq!(approved.detail["recovery"]["operation"], record.id);
    assert_eq!(
        task.current_run().unwrap().gate.as_ref().unwrap().result,
        GateResult::Fail
    );
    assert_eq!(
        task.current_run().unwrap().review.as_ref().unwrap().verdict,
        Some(Verdict::Escalate)
    );
    let receipt = record
        .links
        .iter()
        .find_map(|l| l.acceptance.as_ref())
        .unwrap();
    assert!(!receipt.policy_satisfied);
    assert!(receipt.checks.iter().any(|c| c.observed
        == RecoveryObservation::Gate {
            result: GateResult::Fail
        }
        && c.disposition == CheckDisposition::Overridden));
}
