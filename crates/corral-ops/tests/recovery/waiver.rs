use super::*;

#[test]
fn acceptance_waiver_preserves_missing_review_and_publishes_held_children() {
    if isolated("waiver::acceptance_waiver_preserves_missing_review_and_publishes_held_children") {
        return;
    }
    let plan = PLAN.replace("dependencies: []", "dependencies:\n  - task: first\n    write_scope: [a.rs]\n    depends_on: []\n    acceptance: [{scenario: first, expected: passes}]\n  - task: second\n    write_scope: [b.rs]\n    depends_on: [first]\n    acceptance: [{scenario: second, expected: passes}]").replace("owner_decisions: []", "owner_decisions: [Choose boundary]");
    let f = Fixture::with_plan(false, &plan);
    let notes = "Resolved: Choose boundary => Preserve the closed action set.";
    let mut input = RecoveryInput {
        action: RecoveryAction::AcceptPlan {
            waive_independent_review: true,
            resolution_notes_sha256: crate::verification::digest(notes.as_bytes()),
        },
        exceptions: BTreeSet::from([RecoveryCheck::IndependentPlanReview]),
        notes: notes.into(),
    };
    let preview = recover_preview(&f.ctx, &f.id, &input).unwrap();
    assert!(
        preview.remaining_blockers.is_empty(),
        "{:?}",
        preview.remaining_blockers
    );
    input.notes = "Unresolved".into();
    assert!(
        !recover_preview(&f.ctx, &f.id, &input)
            .unwrap()
            .remaining_blockers
            .is_empty()
    );
    input.notes = notes.into();
    let record = recover_apply(&f.ctx, &preview, &input, Some(confirmation(&preview))).unwrap();
    assert_eq!(
        state(&record).unwrap(),
        RecoveryState::Applied,
        "{:?}",
        record.outcomes
    );
    let parent = f.ctx.store.get(&f.id).unwrap();
    assert_eq!(parent.status, Status::Done);
    assert_eq!(parent.children.len(), 2);
    assert_eq!(parent.plans[0].reviews.len(), 1);
    assert_eq!(parent.plans[0].reviews[0].reviewer, "owner");
    assert_eq!(
        record.preview.checks[0].observed,
        RecoveryObservation::Missing
    );
    assert_eq!(
        record.preview.checks[0].disposition,
        CheckDisposition::Waived
    );
    for child in &parent.children {
        let child = f.ctx.store.get(child).unwrap();
        assert!(child.dispatch_hold && !child.queued);
    }
    // Reopening/reconciling publication never issues another launch or removes a child's hold.
    let mut reopened = f.ctx.clone();
    reopened.store = Store::open_unobserved(f.ctx.store.root()).unwrap();
    let reconciled = recover_reconcile(&reopened, &f.id, &record.id).unwrap();
    assert_eq!(state(&reconciled).unwrap(), RecoveryState::Applied);
    for child in &parent.children {
        assert!(reopened.store.get(child).unwrap().dispatch_hold);
    }
}

#[test]
fn uncertain_intent_retries_never_submit_a_second_command() {
    if isolated("waiver::uncertain_intent_retries_never_submit_a_second_command") {
        return;
    }
    let f = Fixture::new(true);
    let input = f.input();
    let preview = recover_preview(&f.ctx, &f.id, &input).unwrap();
    let confirmation = confirmation(&preview);
    let mut record = RecoveryRecord {
        version: RECOVERY_VERSION,
        id: format!("REC-{}", preview.digest),
        preview: preview.clone(),
        authorization: confirmation.authorization,
        outcomes: vec![
            outcome(RecoveryState::Authorized, "intent", None),
            outcome(
                RecoveryState::Applying,
                "submission may have occurred",
                None,
            ),
        ],
        links: vec![],
    };
    f.ctx.store.save_recovery_record(&f.id, &record).unwrap();
    record = recover_reconcile(&f.ctx, &f.id, &record.id).unwrap();
    assert_eq!(state(&record).unwrap(), RecoveryState::Unknown);
    assert_eq!(
        recover_apply(&f.ctx, &preview, &input, None).unwrap(),
        record
    );
    assert!(crate::plan_review(&f.ctx, &f.id, Some("gpt-astra")).is_err());
    assert!(!f._tmp.path().join("launch-command").exists());
    assert!(f.ctx.store.get(&f.id).unwrap().plans[0].reviews.is_empty());
}
