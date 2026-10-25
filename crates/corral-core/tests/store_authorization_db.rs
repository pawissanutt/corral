//! Authorization is one atomic task, event and record publication before the send callback.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use corral_core::backend::{ChangeSeq, FaultInjector, OpKind, PageReq, RecordQuery, RecordWrite};
use corral_core::sqlite::{SqliteBackend, SqliteOptions};
use corral_core::{
    ActionAuthorization, OperatingActionIdentity, OperatingActionKind, OperatingOrigin, Status,
    Store, SubmissionDisposition, SubmissionOutcome,
};
use serde_json::json;

fn prepared() -> (tempfile::TempDir, Store, ActionAuthorization) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    store
        .edit_config(|doc| doc.set_str("projects.app.path", "."))
        .unwrap();
    let task = store
        .create("Authorization", Some("app"), Status::Ready, "Scope\n")
        .unwrap();
    let now = "2026-10-18T03:00:00Z".parse().unwrap();
    let authorization = store
        .authorize_operating_action(
            OperatingActionIdentity {
                task: task.id,
                run: "R1".into(),
                attempt: "launch-1".into(),
                kind: OperatingActionKind::Dispatch,
                candidate: None,
                plan: None,
                round: None,
            },
            OperatingOrigin::ExplicitOwner {
                approval: "launch-1".into(),
                confirmed_by: "owner".into(),
                confirmed_at: now,
            },
        )
        .unwrap();
    (dir, store, authorization)
}

fn buffer(store: &Store, id: &str) -> anyhow::Result<()> {
    store.log_event(Some(id), "submission-prepared", json!({"marker": true}))?;
    store.put_records(vec![RecordWrite {
        area: "submission".into(),
        key: "reservation".into(),
        expected: None,
        doc: Some(json!({"marker": true})),
        updated: "2026-10-18T03:00:00Z".parse()?,
    }])
}

fn reservation_exists(store: &Store) -> bool {
    !store
        .view()
        .unwrap()
        .records(
            &RecordQuery {
                area: Some("submission".into()),
                key: Some("reservation".into()),
            },
            PageReq::first(1),
        )
        .unwrap()
        .items
        .is_empty()
}

fn assert_one_authorization(store: &Store, seq: ChangeSeq) {
    let changes = store.changes_since(seq, 10).unwrap();
    assert_eq!(changes.entries.len(), 1);
    assert_eq!(changes.entries[0].op, OpKind::Authorization);
}

#[test]
fn send_observes_one_authorization_with_marker_events_and_records() {
    // Given a durable preparation and independently opened handles on the same database.
    let (dir, store, authorization) = prepared();
    let other = Store::open(dir.path()).unwrap();
    let seq = store.view().unwrap().seq();
    let sends = Cell::new(0);
    // When preparation buffers through both the callback handle and another handle.
    let outcome = store
        .submit_operating_action(
            &authorization,
            &authorization.action,
            |fresh, task| {
                assert!(!corral_core::sqlite::guard::in_write_transaction());
                task.queued = true;
                buffer(&other, &task.id)?;
                fresh.log_event(Some(&task.id), "callback-handle", json!({}))?;
                assert!(!reservation_exists(&other));
                assert!(
                    other
                        .events_of(Some(&task.id), &["submission-prepared"])?
                        .is_empty()
                );
                Ok(())
            },
            || {
                sends.set(sends.get() + 1);
                assert!(!corral_core::sqlite::guard::in_write_transaction());
                assert_one_authorization(&other, seq);
                assert!(other.get(&authorization.action.task)?.queued);
                assert!(reservation_exists(&other));
                assert_eq!(
                    other
                        .events_of(Some(&authorization.action.task), &["submission-prepared"])?
                        .len(),
                    1
                );
                assert!(matches!(
                    other.authorized_submission(&authorization.id)?.disposition,
                    SubmissionDisposition::Submitting
                ));
                Ok(17)
            },
        )
        .unwrap();
    // Then completion is its own operation and replay invokes neither callback.
    assert!(matches!(outcome, SubmissionOutcome::Submitted(17)));
    assert_eq!(sends.get(), 1);
    let changes = store.changes_since(seq, 10).unwrap();
    assert_eq!(changes.entries.len(), 2);
    assert!(
        changes
            .entries
            .iter()
            .all(|entry| entry.op == OpKind::Authorization)
    );
    assert!(matches!(
        other
            .authorized_submission(&authorization.id)
            .unwrap()
            .disposition,
        SubmissionDisposition::Submitted
    ));
    assert!(
        store
            .submit_operating_action(
                &authorization,
                &authorization.action,
                |_, _| panic!("replay entered prepare"),
                || -> anyhow::Result<()> { panic!("replay entered send") },
            )
            .is_err()
    );
    assert!(!store.state_dir().join("action-authorizations").exists());
}

#[test]
fn failed_prepare_discards_every_buffered_document_and_only_records_refusal() {
    // Given a prepared action and its exact canonical task.
    let (_dir, store, authorization) = prepared();
    let before = store.get(&authorization.action.task).unwrap();
    let seq = store.view().unwrap().seq();
    // When prepare changes the in-memory marker, buffers documents and then refuses.
    let outcome = store
        .submit_operating_action(
            &authorization,
            &authorization.action,
            |fresh, task| {
                task.queued = true;
                buffer(fresh, &task.id)?;
                anyhow::bail!("identity moved");
            },
            || -> anyhow::Result<()> { panic!("refusal entered send") },
        )
        .unwrap();
    // Then only the refusal is durable; no unit data escapes into later writes.
    assert!(matches!(
        outcome,
        SubmissionOutcome::AuthorizationRefused(_)
    ));
    assert_one_authorization(&store, seq);
    assert_eq!(store.get(&before.id).unwrap(), before);
    assert!(!reservation_exists(&store));
    assert!(
        store
            .events_of(Some(&before.id), &["submission-prepared"])
            .unwrap()
            .is_empty()
    );
    store.log_event(None, "after-refusal", json!({})).unwrap();
    assert!(!reservation_exists(&store));
}

#[test]
fn send_failure_commits_uncertain_without_replaying_the_effect() {
    // Given a prepared action whose transport will fail after the authorization commit.
    let (_dir, store, authorization) = prepared();
    let seq = store.view().unwrap().seq();
    let sends = Cell::new(0);
    // When the single effect attempt returns an error.
    let result = store.submit_operating_action(
        &authorization,
        &authorization.action,
        |fresh, task| {
            task.queued = true;
            buffer(fresh, &task.id)
        },
        || -> anyhow::Result<()> {
            sends.set(sends.get() + 1);
            anyhow::bail!("transport failed");
        },
    );
    // Then uncertainty and the complete committed unit survive, and replay refuses.
    assert!(result.is_err());
    assert_eq!(sends.get(), 1);
    assert!(store.get(&authorization.action.task).unwrap().queued);
    assert!(reservation_exists(&store));
    assert_eq!(store.changes_since(seq, 10).unwrap().entries.len(), 2);
    assert!(matches!(
        store
            .authorized_submission(&authorization.id)
            .unwrap()
            .disposition,
        SubmissionDisposition::Uncertain { .. }
    ));
    assert!(
        store
            .submit_operating_action(
                &authorization,
                &authorization.action,
                |_, _| panic!("uncertain replay entered prepare"),
                || -> anyhow::Result<()> { panic!("uncertain replay entered send") },
            )
            .is_err()
    );
}

fn fault_fixture() -> (
    tempfile::TempDir,
    Store,
    ActionAuthorization,
    Arc<FaultInjector>,
) {
    let (dir, base, authorization) = prepared();
    let faults = Arc::new(FaultInjector::default());
    let backend = SqliteBackend::open(
        base.dir(),
        SqliteOptions {
            faults: Arc::clone(&faults),
            ..SqliteOptions::default()
        },
    )
    .unwrap();
    (
        dir,
        base.with_backend(Box::new(backend)),
        authorization,
        faults,
    )
}

#[test]
fn every_authorization_primitive_failure_preserves_prepared_and_sends_nothing() {
    // Given the exact primitive-write count of a complete authorization with buffered records.
    let (_dir, store, authorization, faults) = fault_fixture();
    let committed = Arc::new(AtomicU64::new(0));
    let seen = Arc::clone(&committed);
    let counter = Arc::clone(&faults);
    let store = store.with_submission_barrier(Arc::new(move |point| {
        if point == "authorization_committed" {
            seen.store(counter.writes(), Ordering::SeqCst);
        }
    }));
    store
        .submit_operating_action(
            &authorization,
            &authorization.action,
            |fresh, task| {
                task.queued = true;
                buffer(fresh, &task.id)
            },
            || Ok(()),
        )
        .unwrap();
    let writes = committed.load(Ordering::SeqCst);
    assert!(writes > 0);
    for nth in 1..=writes {
        let (_dir, store, authorization, faults) = fault_fixture();
        let backend = SqliteBackend::open(store.dir(), SqliteOptions::default()).unwrap();
        let before = corral_core::backend::conformance::snapshot(&backend).unwrap();
        let sends = Cell::new(0);
        faults.arm(nth);
        // When any primitive fails before the authorization commits.
        let result = store.submit_operating_action(
            &authorization,
            &authorization.action,
            |fresh, task| {
                task.queued = true;
                buffer(fresh, &task.id)
            },
            || {
                sends.set(sends.get() + 1);
                Ok(())
            },
        );
        // Then no part of the task, events, records, references or authorization escaped.
        assert!(result.is_err(), "primitive {nth}/{writes}");
        assert_eq!(sends.get(), 0);
        assert_eq!(
            corral_core::backend::conformance::snapshot(&backend).unwrap(),
            before,
            "primitive {nth}/{writes}"
        );
        assert!(matches!(
            store
                .authorized_submission(&authorization.id)
                .unwrap()
                .disposition,
            SubmissionDisposition::Prepared
        ));
    }
}
