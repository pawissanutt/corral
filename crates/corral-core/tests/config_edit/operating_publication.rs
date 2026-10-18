use std::cell::Cell;

use corral_core::{
    ActionAuthorization, ConfigDoc, OperatingMode, OperatingPresentation, OperatingSelection,
    Store, SubmissionDisposition, SubmissionOutcome,
};

use super::on_disk;
use super::operating_reauthorization::{clock, prepared};

fn presentation(store: &Store) -> OperatingPresentation {
    let receipt = OperatingPresentation {
        request_digest: "pending-question".into(),
        delivery: "delivery-1".into(),
        confirmed: true,
        at: clock(),
    };
    store
        .record_operating_presentation("question-1", receipt.clone())
        .unwrap();
    receipt
}

fn refuses_submission(store: &Store, original: &ActionAuthorization) {
    let intents = Cell::new(0);
    let submissions = Cell::new(0);
    let outcome = store
        .submit_operating_action(
            original,
            &original.action,
            |_, task| {
                intents.set(intents.get() + 1);
                task.queued = true;
                Ok(())
            },
            || {
                submissions.set(submissions.get() + 1);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(submissions.get(), 0, "{outcome:?}");
    assert_eq!(intents.get(), 0, "{outcome:?}");
    assert!(matches!(
        outcome,
        SubmissionOutcome::AuthorizationRefused(_)
    ));
    assert!(!store.get(&original.action.task).unwrap().queued);
    let record = store.authorized_submission(&original.id).unwrap();
    assert_eq!(record.authorization, *original);
    assert!(matches!(
        record.disposition,
        SubmissionDisposition::AuthorizationRefused { .. }
    ));
}

#[test]
fn failed_invalidation_does_not_publish_a_generic_grant_change() {
    // Given an existing grant, comment-bearing configuration and a presentation receipt.
    let (_root, store, _original) = prepared();
    let receipt = presentation(&store);
    let before = on_disk(&store);
    let state = store.operating_state().unwrap();
    std::fs::create_dir(store.state_dir().join("automation.json.tmp")).unwrap();
    // When receipt invalidation fails during a generic withdrawal.
    assert!(store.assist_operating_policy().is_err());
    // Then no configuration was published; the caller must handle the rejected withdrawal.
    assert_eq!(on_disk(&store), before);
    assert_eq!(store.operating_state().unwrap(), state);
    assert_eq!(state.presentations["question-1"], receipt);
}

#[test]
fn failed_invalidation_cannot_restore_a_historical_grant_or_submit_after_reload() {
    // Given an old grant and definitely-unsubmitted preparation.
    let (root, store, original) = prepared();
    let granted = store.config_layers().unwrap().config().unwrap();
    let receipt = presentation(&store);
    let approval = store.operating_state().unwrap().approval;
    std::fs::create_dir(store.state_dir().join("automation.json.tmp")).unwrap();
    assert!(store.assist_operating_policy().is_err());
    // A supported raw Assist edit also models the previous writer's interrupted
    // withdrawal: configuration is withdrawn, but its old approval still exists.
    let mut withdrawn = ConfigDoc::parse(&on_disk(&store)).unwrap();
    withdrawn.set_str("automation.mode", "assist").unwrap();
    withdrawn.unset("automation.authorization").unwrap();
    std::fs::write(store.config_path(), withdrawn.to_string()).unwrap();
    let before = on_disk(&store);
    // When a stale generic settings save tries to restore the grant while the fault persists.
    assert!(
        store
            .edit_config(|doc| {
                let current = doc.config()?;
                doc.apply_config_changes(&current, &granted)?;
                doc.set_str("ui.done_max", "19")
            })
            .is_err()
    );
    let reopened = Store::open(root.path()).unwrap();
    let unauthorized =
        reopened.authorize_operating_action(original.action.clone(), original.origin.clone());
    // Then the actual final transaction refuses, with preparation retained and zero IPC.
    refuses_submission(&reopened, &original);
    assert!(unauthorized.is_err());
    assert_eq!(on_disk(&reopened), before);
    assert_eq!(
        reopened.effective_operating_policy().unwrap().mode,
        OperatingMode::Assist
    );
    let state = reopened.operating_state().unwrap();
    assert_eq!(state.approval, approval);
    assert_eq!(state.presentations["question-1"], receipt);
}

#[test]
fn configuration_publication_failure_leaves_approval_durably_invalidated() {
    // Given an active grant and a fault in the subsequent configuration write.
    let (root, store, original) = prepared();
    let granted = store.config_layers().unwrap().config().unwrap();
    let receipt = presentation(&store);
    let before = on_disk(&store);
    let fault = store.config_path().with_extension("toml.tmp");
    std::fs::create_dir(&fault).unwrap();
    // When generic withdrawal cannot publish its configuration.
    assert!(store.assist_operating_policy().is_err());
    assert_eq!(on_disk(&store), before);
    let invalidated = store.operating_state().unwrap();
    assert!(invalidated.approval.is_none());
    assert_eq!(invalidated.presentations["question-1"], receipt);
    std::fs::remove_dir(fault).unwrap();
    // A later generic save can retain unrelated values but cannot mint the old approval.
    store
        .edit_config(|doc| {
            let current = doc.config()?;
            doc.apply_config_changes(&current, &granted)?;
            doc.set_str("ui.done_max", "19")
        })
        .unwrap();
    let reopened = Store::open(root.path()).unwrap();
    // Then even matching historical configuration cannot authorize or submit automatically.
    refuses_submission(&reopened, &original);
    assert_eq!(reopened.config.ui.done_max, 19);
    assert_eq!(
        reopened.effective_operating_policy().unwrap().mode,
        OperatingMode::Assist
    );
    assert_eq!(reopened.operating_state().unwrap(), invalidated);
}

#[test]
fn explicit_confirmation_publishes_new_identity_before_its_final_approval() {
    // Given a previous grant, presentation and a newly validated finite preview.
    let (root, store, original) = prepared();
    let receipt = presentation(&store);
    let approval = store.operating_state().unwrap().approval;
    let preview = store
        .preview_operating_policy(&OperatingSelection {
            tasks: [original.action.task.clone()].into_iter().collect(),
            authority: store
                .config_layers()
                .unwrap()
                .config()
                .unwrap()
                .automation
                .authority,
            ..Default::default()
        })
        .unwrap();
    std::fs::create_dir(store.state_dir().join("automation.json.tmp")).unwrap();
    // When the final explicit confirmation receipt cannot be published.
    assert!(
        store
            .confirm_operating_policy(&preview, "owner", clock())
            .is_err()
    );
    let reopened = Store::open(root.path()).unwrap();
    // Then the new configuration exists but the old approval cannot match or submit.
    assert_eq!(reopened.config.automation.mode, OperatingMode::Autopilot);
    assert_eq!(
        reopened.config.automation.authorization.as_ref(),
        Some(&preview.id)
    );
    let state = reopened.operating_state().unwrap();
    assert_eq!(state.approval, approval);
    assert_eq!(state.presentations["question-1"], receipt);
    refuses_submission(&reopened, &original);
    assert_eq!(
        reopened.effective_operating_policy().unwrap().mode,
        OperatingMode::Assist
    );
}
