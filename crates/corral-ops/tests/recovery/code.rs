use super::*;

#[test]
fn missing_receipts_are_hard_and_completed_failed_checks_remain_failed() {
    if isolated("code::missing_receipts_are_hard_and_completed_failed_checks_remain_failed") {
        return;
    }
    let mut f = crate::submission_paths_tests::fixture::Fx::new();
    f.ctx.store.config.review.gate = Some(vec!["false".into()]);
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
    crate::gate_start(&f.ctx, &id).unwrap();
    let gate = crate::gate_collect(&f.ctx, &id).unwrap().unwrap();
    assert_eq!(gate.result, GateResult::Fail);
    crate::verdict(
        &f.ctx,
        &id,
        Verdict::Escalate,
        "Actual completed review: owner intervention needed",
    )
    .unwrap();
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
