use super::Store;

#[test]
fn completed_store_lock_does_not_leave_peer_changing() {
    // Given a duplicated open-file description, as retained across a concurrent fork.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let lock = store.lock("T-002").unwrap();
    let inherited = lock.0.try_clone().unwrap();
    assert!(store.try_lock("T-002").unwrap().is_none());

    // When the store operation finishes while the duplicate descriptor remains open.
    drop(lock);

    // Then a landing check sees the peer as settled, without waiting for the duplicate.
    let next = store.try_lock("T-002").unwrap();
    assert!(
        next.is_some(),
        "completed peer still appears to be changing"
    );
    drop(inherited);
}

#[test]
fn active_store_lock_still_blocks_peer_landing() {
    // Given an active store operation and a duplicate descriptor.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let lock = store.lock("T-002").unwrap();
    let inherited = lock.0.try_clone().unwrap();

    // When only the duplicate closes, while the operation still owns its lock.
    drop(inherited);

    // Then landing remains excluded until the actual operation finishes.
    assert!(store.try_lock("T-002").unwrap().is_none());
    drop(lock);
}
#[test]
fn dropping_owner_unlocks_even_when_child_keeps_an_inherited_descriptor() {
    // Given a descriptor duplicated as it would be across fork before exec.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let guard = store.try_lock("launch-test").unwrap().unwrap();
    let inherited = guard.0.try_clone().unwrap();
    assert!(store.try_lock("launch-test").unwrap().is_none());

    // When the parent finishes normally, the child may still hold its descriptor.
    drop(guard);

    // Then the next owner does not depend on the child's scheduling/exec timing.
    let next = store.try_lock("launch-test").unwrap();
    assert!(next.is_some());
    drop(inherited);
}

#[test]
fn shared_inventory_leases_exclude_claims_until_the_last_consumer_finishes() {
    // Given two consumers retaining the same ownership boundary.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let first = store.try_lock_shared("session-ownership").unwrap().unwrap();
    let second = store.try_lock_shared("session-ownership").unwrap().unwrap();
    assert!(store.try_lock("session-ownership").unwrap().is_none());

    // When one finishes, the other still excludes new ownership.
    drop(first);
    assert!(store.try_lock("session-ownership").unwrap().is_none());
    drop(second);

    // Then an exclusive admission excludes readers until its claim is saved.
    let claim = store.try_lock("session-ownership").unwrap().unwrap();
    assert!(
        store
            .try_lock_shared("session-ownership")
            .unwrap()
            .is_none()
    );
    drop(claim);
    assert!(
        store
            .try_lock_shared("session-ownership")
            .unwrap()
            .is_some()
    );
}

fn prepared_operating_action() -> (tempfile::TempDir, Store, crate::ActionAuthorization) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    store
        .edit_config(|doc| doc.set_str("projects.app.path", "."))
        .unwrap();
    let task = store
        .create("Scoped", Some("app"), crate::Status::Ready, "Scope\n")
        .unwrap();
    let selection = crate::OperatingSelection {
        tasks: std::collections::BTreeSet::from([task.id.clone()]),
        authority: crate::OperatingAuthority {
            dispatch: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let preview = store.preview_operating_policy(&selection).unwrap();
    store
        .confirm_operating_policy(&preview, "owner", "2026-10-18T03:00:00Z".parse().unwrap())
        .unwrap();
    let authorization = store
        .authorize_operating_action(
            crate::OperatingActionIdentity {
                task: task.id,
                run: "R1".into(),
                attempt: "launch-1".into(),
                kind: crate::OperatingActionKind::Dispatch,
                candidate: None,
                plan: None,
                round: None,
            },
            crate::OperatingOrigin::Automatic {
                authorization: preview.id,
                revision: preview.policy.revision,
            },
        )
        .unwrap();
    (root, store, authorization)
}

#[test]
fn policy_writer_cannot_publish_between_intent_and_single_ipc_attempt() {
    use std::sync::mpsc;
    use std::time::Duration;

    // Given subscriptions for IPC entry/release and a competing writer's lock observation.
    let (_root, store, authorization) = prepared_operating_action();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (blocked_tx, blocked_rx) = mpsc::channel();
    let launcher = Store::open(store.root()).unwrap();
    let writer = Store::open(store.root()).unwrap();
    let launched = authorization.clone();
    let thread = std::thread::spawn(move || {
        launcher.submit_operating_action(
            &launched,
            &launched.action,
            |fresh, task| {
                assert!(fresh.try_lock("dispatch")?.is_none());
                assert!(fresh.try_lock(&task.id)?.is_none());
                task.queued = true;
                Ok(())
            },
            || {
                // The durable intent and task mutation must already be visible at IPC entry.
                assert!(launcher.get(&launched.action.task)?.queued);
                assert!(matches!(
                    launcher.authorized_submission(&launched.id)?.disposition,
                    crate::SubmissionDisposition::Submitting
                ));
                entered_tx.send(())?;
                release_rx.recv_timeout(Duration::from_secs(10))?;
                Ok(())
            },
        )
    });
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    // When a second context requests revocation while IPC owns the board boundary.
    let writer_thread = std::thread::spawn(move || {
        assert!(writer.try_lock("dispatch").unwrap().is_none());
        blocked_tx.send(()).unwrap();
        writer.pause_operating_policy(true)
    });
    blocked_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    release_tx.send(()).unwrap();
    // Then one in-flight submission finishes before the writer can publish its pause.
    assert!(matches!(
        thread.join().unwrap().unwrap(),
        crate::SubmissionOutcome::Submitted(())
    ));
    assert!(writer_thread.join().unwrap().unwrap().automation.paused);
    assert!(matches!(
        store
            .authorized_submission(&authorization.id)
            .unwrap()
            .disposition,
        crate::SubmissionDisposition::Submitted
    ));
    assert!(!store.effective_operating_policy().unwrap().allows(
        &authorization.action.task,
        crate::OperatingActionKind::Dispatch
    ));
}

#[test]
fn revocation_completed_before_boundary_refuses_without_saving_submission_intent() {
    use std::sync::mpsc;
    use std::time::Duration;

    // Given an executor paused outside the final boundary after durable preparation.
    let (_root, store, authorization) = prepared_operating_action();
    let (arrived_tx, arrived_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let launcher = Store::open(store.root()).unwrap();
    let launched = authorization.clone();
    let thread = std::thread::spawn(move || {
        arrived_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        launcher.submit_operating_action(
            &launched,
            &launched.action,
            |_, _| panic!("revoked action entered submission intent"),
            || -> anyhow::Result<()> { panic!("revoked action entered IPC") },
        )
    });
    arrived_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    // When a fresh context publishes Assist before the executor is released.
    Store::open(store.root())
        .unwrap()
        .assist_operating_policy()
        .unwrap();
    release_tx.send(()).unwrap();
    // Then it records a definite refusal with unchanged preparation, not uncertainty.
    assert!(matches!(
        thread.join().unwrap().unwrap(),
        crate::SubmissionOutcome::AuthorizationRefused(_)
    ));
    assert!(!store.get(&authorization.action.task).unwrap().queued);
    let record = store.authorized_submission(&authorization.id).unwrap();
    assert_eq!(record.authorization, authorization);
    assert!(matches!(
        record.disposition,
        crate::SubmissionDisposition::AuthorizationRefused { .. }
    ));
    assert!(store.try_lock("dispatch").unwrap().is_some());
}

#[test]
fn fresh_executor_identity_refusal_preserves_the_prepared_task_and_never_submits() {
    // Given a prepared action whose executor journal changed before its final check.
    let (_root, store, authorization) = prepared_operating_action();
    let before = std::fs::read(store.get(&authorization.action.task).unwrap().path).unwrap();
    // When the executor rejects the exact fresh attempt inside the short boundary.
    let result = store
        .submit_operating_action(
            &authorization,
            &authorization.action,
            |_, task| {
                task.queued = true;
                anyhow::bail!("executor attempt no longer matches")
            },
            || -> anyhow::Result<()> { panic!("stale executor identity submitted") },
        )
        .unwrap();
    // Then it is definitely not submitted and even the in-memory marker was not saved.
    assert!(matches!(
        result,
        crate::SubmissionOutcome::AuthorizationRefused(_)
    ));
    assert_eq!(
        std::fs::read(store.get(&authorization.action.task).unwrap().path).unwrap(),
        before
    );
    assert!(matches!(
        store
            .authorized_submission(&authorization.id)
            .unwrap()
            .disposition,
        crate::SubmissionDisposition::AuthorizationRefused { .. }
    ));
}

mod operating_submission {
    use std::collections::BTreeSet;
    use std::fs;

    use crate::{
        OperatingActionIdentity, OperatingActionKind, OperatingAuthority, OperatingMode,
        OperatingOrigin, OperatingSelection, Status, Store, SubmissionDisposition,
        SubmissionOutcome, Task,
    };
    use jiff::Timestamp;
    use tempfile::TempDir;

    struct Fixture {
        root: TempDir,
        store: Store,
        task: Task,
    }

    fn clock() -> Timestamp {
        "2026-10-18T03:00:00Z".parse().unwrap()
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            fs::create_dir_all(root.path().join("app/.git")).unwrap();
            let store = Store::init(root.path()).unwrap();
            let task = store
                .create("Included", Some("app"), Status::Backlog, "Agreed work\n")
                .unwrap();
            Self { root, store, task }
        }

        fn selection(&self) -> OperatingSelection {
            OperatingSelection {
                tasks: BTreeSet::from([self.task.id.clone()]),
                authority: OperatingAuthority {
                    dispatch: true,
                    accept: true,
                    plan_accept: true,
                    aggregate_accept: false,
                },
                ..OperatingSelection::default()
            }
        }

        fn grant(&self) {
            let preview = self
                .store
                .preview_operating_policy(&self.selection())
                .unwrap();
            self.store
                .confirm_operating_policy(&preview, "owner", clock())
                .unwrap();
        }

        fn action(&self) -> OperatingActionIdentity {
            OperatingActionIdentity {
                task: self.task.id.clone(),
                run: "R1".into(),
                attempt: "launch-1".into(),
                kind: OperatingActionKind::Dispatch,
                candidate: None,
                plan: None,
                round: None,
            }
        }

        fn automatic(&self) -> crate::ActionAuthorization {
            let effective = self.store.effective_operating_policy().unwrap();
            self.store
                .authorize_operating_action(
                    self.action(),
                    OperatingOrigin::Automatic {
                        authorization: effective.authorization.unwrap(),
                        revision: self
                            .store
                            .config_layers()
                            .unwrap()
                            .config()
                            .unwrap()
                            .automation
                            .revision,
                    },
                )
                .unwrap()
        }
    }

    #[test]
    fn preceding_revocation_refuses_without_preparation_or_ipc_and_retains_identity() {
        for revocation in ["pause", "assist", "scope", "authority", "malformed"] {
            // Given a durable automatic authorization prepared before another context revokes it.
            let fixture = Fixture::new();
            fixture.grant();
            let authorization = fixture.automatic();
            let other = Store::open(fixture.root.path()).unwrap();
            match revocation {
                "pause" => {
                    other.pause_operating_policy(true).unwrap();
                }
                "assist" => {
                    other.assist_operating_policy().unwrap();
                }
                "scope" => {
                    other
                        .edit_config(|doc| doc.unset("automation.scope.tasks").map(|_| ()))
                        .unwrap();
                }
                "authority" => {
                    other
                        .edit_config(|doc| doc.set_str("automation.authority.dispatch", "false"))
                        .unwrap();
                }
                "malformed" => {
                    fs::write(other.config_path(), "invalid [").unwrap();
                }
                _ => unreachable!(),
            }
            let before = fs::read(&fixture.task.path).unwrap();
            // When the final boundary is reached with the original origin and action.
            let result = fixture
                .store
                .submit_operating_action(
                    &authorization,
                    &fixture.action(),
                    |_, _| panic!("refused authorization entered task submission state"),
                    || -> anyhow::Result<()> { panic!("refused authorization submitted IPC") },
                )
                .unwrap();
            // Then preparation is unchanged and refusal is definite, not uncertainty.
            assert!(
                matches!(result, SubmissionOutcome::AuthorizationRefused(_)),
                "{revocation}"
            );
            assert_eq!(fs::read(&fixture.task.path).unwrap(), before);
            let record = fixture
                .store
                .authorized_submission(&authorization.id)
                .unwrap();
            assert_eq!(record.authorization, authorization);
            assert!(matches!(
                record.disposition,
                SubmissionDisposition::AuthorizationRefused { .. }
            ));
        }
    }

    #[test]
    fn explicit_owner_action_in_assist_is_exact_and_never_approves_the_next_action() {
        // Given one persisted owner approval in Assist.
        let fixture = Fixture::new();
        let action = fixture.action();
        let authorization = fixture
            .store
            .authorize_operating_action(
                action.clone(),
                OperatingOrigin::ExplicitOwner {
                    approval: "owner-start-1".into(),
                    confirmed_by: "owner".into(),
                    confirmed_at: clock(),
                },
            )
            .unwrap();
        // When the exact action submits after its task intent is saved.
        let result = fixture
            .store
            .submit_operating_action(
                &authorization,
                &action,
                |_, task| {
                    task.queued = true;
                    Ok(())
                },
                || {
                    assert!(fixture.store.get(&fixture.task.id)?.queued);
                    assert!(matches!(
                        fixture
                            .store
                            .authorized_submission(&authorization.id)?
                            .disposition,
                        SubmissionDisposition::Submitting
                    ));
                    Ok(7)
                },
            )
            .unwrap();
        // Then it works once, but another attempt and an automatic caller cannot reuse it.
        assert!(matches!(result, SubmissionOutcome::Submitted(7)));
        let mut next = action.clone();
        next.attempt = "launch-2".into();
        assert!(
            fixture
                .store
                .submit_operating_action(&authorization, &next, |_, _| Ok(()), || Ok(()))
                .is_err()
        );
        assert!(
            fixture
                .store
                .submit_operating_action(&authorization, &action, |_, _| Ok(()), || Ok(()))
                .is_err()
        );
        assert!(
            fixture
                .store
                .authorize_operating_action(
                    next,
                    OperatingOrigin::Automatic {
                        authorization: "invented".into(),
                        revision: 0,
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn uncertain_ipc_is_retained_and_never_replayed() {
        // Given a prepared automatic action.
        let fixture = Fixture::new();
        fixture.grant();
        let authorization = fixture.automatic();
        // When IPC returns an error after entering the submission boundary.
        assert!(
            fixture
                .store
                .submit_operating_action(
                    &authorization,
                    &fixture.action(),
                    |_, _| Ok(()),
                    || -> anyhow::Result<()> { anyhow::bail!("transport disconnected") }
                )
                .is_err()
        );
        // Then uncertainty survives reopening and cannot be replayed.
        let reopened = Store::open(fixture.root.path()).unwrap();
        assert!(matches!(
            reopened
                .authorized_submission(&authorization.id)
                .unwrap()
                .disposition,
            SubmissionDisposition::Uncertain { .. }
        ));
        assert!(
            reopened
                .submit_operating_action(
                    &authorization,
                    &fixture.action(),
                    |_, _| Ok(()),
                    || -> anyhow::Result<()> { panic!("uncertain action replayed") }
                )
                .is_err()
        );
    }

    #[test]
    fn legacy_missing_or_forged_provenance_cannot_submit() {
        // Given a valid authorization with no possible owner fallback.
        let fixture = Fixture::new();
        fixture.grant();
        let authorization = fixture.automatic();
        // When a caller changes its origin without a matching persisted approval.
        let mut forged = authorization.clone();
        forged.origin = OperatingOrigin::ExplicitOwner {
            approval: "forged".into(),
            confirmed_by: "owner".into(),
            confirmed_at: clock(),
        };
        // Then no IPC can be submitted from forged or missing provenance.
        assert!(
            fixture
                .store
                .submit_operating_action(
                    &forged,
                    &fixture.action(),
                    |_, _| Ok(()),
                    || -> anyhow::Result<()> { panic!("forged authority submitted") }
                )
                .is_err()
        );
        fs::remove_file(
            fixture
                .store
                .state_dir()
                .join("action-authorizations")
                .join(format!("{}.json", authorization.id)),
        )
        .unwrap();
        assert!(
            fixture
                .store
                .submit_operating_action(
                    &authorization,
                    &fixture.action(),
                    |_, _| Ok(()),
                    || -> anyhow::Result<()> { panic!("missing authority submitted") }
                )
                .is_err()
        );
    }

    #[test]
    fn malformed_or_mismatched_approval_records_cannot_authorize() {
        for change in ["workspace", "revision", "digest", "version", "missing"] {
            // Given a valid confirmed approval.
            let fixture = Fixture::new();
            fixture.grant();
            let path = fixture.store.state_dir().join("automation.json");
            let mut state = serde_json::to_value(fixture.store.operating_state().unwrap()).unwrap();
            // When only one receipt binding is corrupted.
            match change {
                "workspace" => state["approval"]["workspace"] = "/different".into(),
                "revision" => state["approval"]["revision"] = 99.into(),
                "digest" => state["approval"]["digest"] = "different".into(),
                "version" => state["version"] = 99.into(),
                "missing" => {
                    state["approval"]
                        .as_object_mut()
                        .unwrap()
                        .remove("authorization");
                }
                _ => unreachable!(),
            }
            fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
            // Then either decoding refuses or the effective result is Assist, never Autopilot.
            let result = fixture.store.effective_operating_policy();
            assert!(
                result.is_err() || result.unwrap().mode == OperatingMode::Assist,
                "{change}"
            );
        }
    }

    #[test]
    fn raw_autopilot_configuration_without_confirmation_is_not_authority() {
        // Given a workspace with no approval receipt.
        let fixture = Fixture::new();
        let preview = fixture
            .store
            .preview_operating_policy(&fixture.selection())
            .unwrap();
        // When a raw edit copies even an otherwise valid prospective configuration.
        fixture
            .store
            .edit_config(|doc| {
                let before = doc.config()?;
                let mut draft = before.clone();
                draft.automation = preview.policy.clone();
                doc.apply_config_changes(&before, &draft)
            })
            .unwrap();
        // Then the requested mode is readable, but the effective mode is Assist.
        assert_eq!(
            fixture
                .store
                .config_layers()
                .unwrap()
                .config()
                .unwrap()
                .automation
                .mode,
            OperatingMode::Autopilot
        );
        assert_eq!(
            fixture.store.effective_operating_policy().unwrap().mode,
            OperatingMode::Assist
        );
        assert!(fixture.store.operating_state().unwrap().approval.is_none());
    }

    #[test]
    fn partial_publication_cannot_activate_without_the_final_approval_receipt() {
        // Given a confirmation whose final atomic receipt write will fail.
        let fixture = Fixture::new();
        let preview = fixture
            .store
            .preview_operating_policy(&fixture.selection())
            .unwrap();
        fs::create_dir(fixture.store.state_dir().join("automation.json.tmp")).unwrap();
        // When configuration publication succeeds but receipt publication fails.
        assert!(
            fixture
                .store
                .confirm_operating_policy(&preview, "owner", clock())
                .is_err()
        );
        // Then the on-disk requested mode remains inspectable and inactive.
        let reopened = Store::open(fixture.root.path()).unwrap();
        assert_eq!(reopened.config.automation.mode, OperatingMode::Autopilot);
        assert_eq!(
            reopened.effective_operating_policy().unwrap().mode,
            OperatingMode::Assist
        );
    }

    #[test]
    fn restored_pause_reuses_prepared_identity_without_replaying_an_attempt() {
        // Given a prepared action definitely refused during a pause.
        let fixture = Fixture::new();
        fixture.grant();
        let authorization = fixture.automatic();
        fixture.store.pause_operating_policy(true).unwrap();
        assert!(matches!(
            fixture
                .store
                .submit_operating_action(
                    &authorization,
                    &fixture.action(),
                    |_, _| panic!("paused action prepared submission"),
                    || -> anyhow::Result<()> { panic!("paused action submitted") }
                )
                .unwrap(),
            SubmissionOutcome::AuthorizationRefused(_)
        ));
        // When the same valid grant resumes and its original attempt is admitted.
        fixture.store.pause_operating_policy(false).unwrap();
        assert_eq!(fixture.automatic(), authorization);
        let result = fixture
            .store
            .submit_operating_action(&authorization, &fixture.action(), |_, _| Ok(()), || Ok(()))
            .unwrap();
        // Then its original preparation submits exactly once.
        assert!(matches!(result, SubmissionOutcome::Submitted(())));
        assert!(
            fixture
                .store
                .submit_operating_action(
                    &authorization,
                    &fixture.action(),
                    |_, _| Ok(()),
                    || -> anyhow::Result<()> { panic!("attempt replayed") }
                )
                .is_err()
        );
    }

    #[test]
    fn new_owner_origin_cannot_reauthorize_the_same_automatic_attempt() {
        // Given an exact attempt with persisted automatic provenance.
        let fixture = Fixture::new();
        fixture.grant();
        let authorization = fixture.automatic();
        // When a caller attempts to turn that identity into an owner action.
        assert!(
            fixture
                .store
                .authorize_operating_action(
                    fixture.action(),
                    OperatingOrigin::ExplicitOwner {
                        approval: "owner-approval".into(),
                        confirmed_by: "owner".into(),
                        confirmed_at: clock()
                    }
                )
                .is_err()
        );
        // Then the original provenance remains durable; another origin is not another IPC slot.
        assert_eq!(
            fixture
                .store
                .authorized_submission(&authorization.id)
                .unwrap()
                .authorization,
            authorization
        );
    }
}
