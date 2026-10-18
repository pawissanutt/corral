use super::*;

#[test]
fn setup_journal_preserves_history_and_rejects_stale_writers() {
    // Given an empty isolated store.
    let (dir, store, _task) = store_fixture();
    assert_eq!(
        store.profile_setup().unwrap(),
        ProfileSetupRecord::default()
    );
    let gathering = ProfileSetupRecord {
        revision: 1,
        state: ProfileSetupState::Gathering,
        ..Default::default()
    };
    store.save_profile_setup(&gathering).unwrap();
    // When a proposed state is appended and the store reopened.
    let proposed = ProfileSetupRecord {
        revision: 2,
        state: ProfileSetupState::Proposed,
        proposal: Some(proposal()),
        ..Default::default()
    };
    store.save_profile_setup(&proposed).unwrap();
    store.save_profile_setup(&proposed).unwrap();
    let reopened = Store::open(dir.path()).unwrap();
    // Then a duplicate is not another transition and stale writes cannot reset setup.
    assert_eq!(
        reopened.profile_setup_history().unwrap(),
        vec![gathering.clone(), proposed.clone()]
    );
    assert!(reopened.save_profile_setup(&gathering).is_err());
    assert_eq!(reopened.profile_setup().unwrap(), proposed);
}

#[test]
fn auto_journal_is_idempotent_and_rejects_conflicting_receipt_identity() {
    // Given one valid unpinned task selection.
    let (dir, store, task) = store_fixture();
    let receipt = auto_selection(&store.config, &task);
    store.save_auto_profile_selection(&receipt, None).unwrap();
    // When the exact receipt is redelivered after reopening.
    let reopened = Store::open(dir.path()).unwrap();
    reopened
        .save_auto_profile_selection(&receipt, None)
        .unwrap();
    // Then only one receipt exists and reusing its ID with different content is rejected.
    assert_eq!(
        reopened.auto_profile_selections().unwrap(),
        vec![receipt.clone()]
    );
    let mut conflict = receipt;
    conflict.actor = SelectionActor::Pm;
    assert!(
        reopened
            .save_auto_profile_selection(&conflict, None)
            .is_err()
    );
    assert!(reopened.get(&task.id).unwrap().profile.is_none());
}

#[test]
fn repair_journal_survives_restart_and_refuses_counter_reset() {
    // Given a persisted initial attempt.
    let (dir, store, task) = store_fixture();
    let mut history =
        RepairHistory::new(RepairPolicy::approved_starter_policy(), "omo", None).unwrap();
    let initial = RepairEvent::Started {
        receipt: RepairStartReceipt {
            attempt_id: RepairAttemptId("R1".into()),
            profile: "omo".into(),
            kind: RepairAttemptKind::InitialImplementation,
            findings: vec![],
        },
    };
    history.apply(initial).unwrap();
    store
        .save_profile_repair_history(&task.id, &history)
        .unwrap();
    // When reopening and attempting to replace it with an empty allowance.
    let reopened = Store::open(dir.path()).unwrap();
    let empty = RepairHistory::new(RepairPolicy::approved_starter_policy(), "omo", None).unwrap();
    let result = reopened.save_profile_repair_history(&task.id, &empty);
    // Then durable counters and history are preserved.
    assert!(result.is_err());
    let saved = reopened.profile_repair_histories().unwrap();
    assert_eq!(saved[&task.id], history);
    assert_eq!(
        saved[&task.id].counters().unwrap().initial_implementations,
        1
    );
    assert_eq!(saved[&task.id].counters().unwrap().coding_repairs, 0);
}

#[test]
fn journals_reject_future_versions_without_modifying_configuration() {
    // Given a future-version document in an isolated store.
    let (_dir, store, _task) = store_fixture();
    let before = store.config.clone();
    std::fs::write(
        store.state_dir().join("profile-setup.json"),
        r#"{"version":99,"records":[]}"#,
    )
    .unwrap();
    // When reading that unsupported journal.
    let result = store.profile_setup();
    // Then no unsupported state is interpreted or active configuration mutated.
    assert!(result.is_err());
    assert_eq!(store.config, before);
}
