//! T-322: callback records, task rows and events share one commit.

mod store_support;

use std::sync::Arc;

use corral_core::backend::{FaultInjector, PageReq, RecordQuery, RecordWrite, StoredRecord};
use corral_core::sqlite::{SqliteBackend, SqliteOptions};
use corral_core::{Status, Store};
use serde_json::json;

fn write(key: &str, expected: Option<u64>) -> RecordWrite {
    RecordWrite {
        area: "atomic-unit".into(),
        key: key.into(),
        expected,
        doc: Some(json!({"marker": key})),
        updated: store_support::T0.parse().unwrap(),
    }
}

fn records(store: &Store) -> Vec<StoredRecord> {
    store
        .view()
        .unwrap()
        .records(
            &RecordQuery {
                area: Some("atomic-unit".into()),
                key: None,
            },
            PageReq::first(100),
        )
        .unwrap()
        .items
}

fn change(store: &Store, id: &str, head: bool) -> anyhow::Result<()> {
    let callback = || {
        assert!(!corral_core::sqlite::guard::in_write_transaction());
        assert!(store.try_lock("dispatch")?.is_none());
        assert!(store.try_lock(id)?.is_none());
        store.put_records(vec![write("committed", None)])?;
        store.log_event(Some(id), "atomic-unit", json!({"record": "committed"}))
    };
    if head {
        store.update_head(id, |task| {
            task.rounds += 1;
            task.title.push_str(" updated");
            callback()
        })?;
    } else {
        store.update(id, |task| {
            task.rounds += 1;
            task.title.push_str(" updated");
            callback()
        })?;
    }
    Ok(())
}

#[test]
fn full_and_head_updates_commit_records_with_task_and_events() {
    for head in [false, true] {
        // Given a task and an observer that predates the callback.
        let (dir, store) = store_support::store();
        let task = store
            .create("Atomic unit", None, Status::Backlog, "")
            .unwrap();
        let seq = store.view().unwrap().seq();
        let pinned = store.read_only_snapshot().unwrap();
        // When the callback changes all three document families.
        change(&store, &task.id, head).unwrap();
        // Then observers see one commit, never a partial publication.
        assert_eq!(store.changes_since(seq, 10).unwrap().entries.len(), 1);
        assert_eq!(pinned.get(&task.id).unwrap(), task);
        assert!(records(&pinned).is_empty());
        let reopened = Store::open(dir.path()).unwrap();
        assert_eq!(reopened.get(&task.id).unwrap().rounds, task.rounds + 1);
        assert_eq!(records(&reopened)[0].doc, json!({"marker": "committed"}));
        assert_eq!(
            reopened
                .events_of(Some(&task.id), &["edited", "atomic-unit"])
                .unwrap()
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            ["edited", "atomic-unit"],
            "facade audits precede the callback's semantic events; head={head}"
        );
        assert_eq!(
            reopened
                .events_of(Some(&task.id), &["atomic-unit"])
                .unwrap()
                .len(),
            1
        );
    }
}

#[test]
fn records_from_other_handles_join_and_are_discarded_on_false_or_error() {
    for head in [false, true] {
        for rejected in [false, true] {
            // Given independent handles to one database.
            let (dir, store) = store_support::store();
            let other = Store::open(dir.path()).unwrap();
            let task = store
                .create("Discard unit", None, Status::Backlog, "")
                .unwrap();
            let seq = store.view().unwrap().seq();
            let callback = || -> anyhow::Result<bool> {
                other.put_records(vec![write("existing", None)])?;
                Store::open(dir.path())?.put_records(vec![write("reopened", None)])?;
                assert!(records(&other).is_empty(), "no early publication");
                store.log_event(Some(&task.id), "atomic-unit", json!({}))?;
                if rejected {
                    anyhow::bail!("reject callback");
                }
                Ok(false)
            };
            // When the callback declines publication.
            let result = if head {
                store
                    .try_update_head(&task.id, |_| callback())
                    .map(|task| task.is_some())
            } else {
                store
                    .try_update(&task.id, |_| callback())
                    .map(|task| task.is_some())
            };
            // Then its records and events are absent and cannot leak into a later operation.
            if rejected {
                assert!(result.is_err());
            } else {
                assert!(!result.unwrap());
            }
            assert_eq!(store.get(&task.id).unwrap(), task);
            assert_eq!(store.view().unwrap().seq(), seq);
            assert!(records(&other).is_empty());
            store.put_records(vec![write("later", None)]).unwrap();
            assert_eq!(records(&store).len(), 1);
            assert_eq!(records(&store)[0].key, "later");
        }
    }
}

#[test]
fn stale_record_version_refuses_the_entire_task_unit() {
    for head in [false, true] {
        // Given a record whose version no longer matches the callback's precondition.
        let (_dir, store) = store_support::store();
        let task = store
            .create("Conflict unit", None, Status::Backlog, "")
            .unwrap();
        store.put_records(vec![write("committed", None)]).unwrap();
        let before = corral_core::backend::conformance::snapshot(
            &SqliteBackend::open(store.dir(), SqliteOptions::default()).unwrap(),
        )
        .unwrap();
        // When a task change tries to insert the existing record again.
        assert!(change(&store, &task.id, head).is_err());
        // Then even the task and event writes before the record conflict roll back.
        let after = corral_core::backend::conformance::snapshot(
            &SqliteBackend::open(store.dir(), SqliteOptions::default()).unwrap(),
        )
        .unwrap();
        assert_eq!(after, before);
    }
}

fn fixture() -> (tempfile::TempDir, Store, Arc<FaultInjector>, String) {
    let (dir, base) = store_support::store();
    let faults = Arc::new(FaultInjector::default());
    let backend = SqliteBackend::open(
        base.dir(),
        SqliteOptions {
            faults: Arc::clone(&faults),
            ..SqliteOptions::default()
        },
    )
    .unwrap();
    let store = base.with_backend(Box::new(backend));
    let task = store
        .create("Fault unit", None, Status::Backlog, "")
        .unwrap();
    (dir, store, faults, task.id)
}

#[test]
fn every_primitive_failure_rolls_back_the_complete_callback_unit() {
    for head in [false, true] {
        let (_dir, store, faults, id) = fixture();
        faults.reset_count();
        change(&store, &id, head).unwrap();
        let writes = faults.writes();
        assert!(writes > 0);
        for nth in 1..=writes {
            // Given an independently initialized store and one armed primitive failure.
            let (_dir, store, faults, id) = fixture();
            let backend = SqliteBackend::open(store.dir(), SqliteOptions::default()).unwrap();
            let before = corral_core::backend::conformance::snapshot(&backend).unwrap();
            faults.arm(nth);
            // When that write fails inside the task unit.
            assert!(change(&store, &id, head).is_err(), "head={head}, nth={nth}");
            // Then task, records, events, aggregates and change entry all remain absent.
            let after = corral_core::backend::conformance::snapshot(&backend).unwrap();
            assert_eq!(after, before, "head={head}, nth={nth}");
        }
    }
}

#[test]
fn callback_audits_use_the_committed_task_update_stamp() {
    for head in [false, true] {
        // Given a task whose previous timestamp is fixed, not a timing-dependent fixture.
        let (_dir, store) = store_support::store();
        let mut task = store
            .create("Audit stamp", None, Status::Backlog, "")
            .unwrap();
        task.updated = store_support::T0.parse().unwrap();
        store.fixture_put_task(&task).unwrap();
        // When a callback includes its task update stamp in a machine-consumed audit.
        if head {
            store
                .update_head(&task.id, |current| {
                    current.rounds += 1;
                    store.log_event(
                        Some(&current.id),
                        "moved",
                        json!({"task_updated": current.updated}),
                    )
                })
                .unwrap();
        } else {
            store
                .update(&task.id, |current| {
                    current.rounds += 1;
                    store.log_event(
                        Some(&current.id),
                        "moved",
                        json!({"task_updated": current.updated}),
                    )
                })
                .unwrap();
        }
        // Then the audit joins the saved task and related decision audits by the same stamp.
        let saved = store.get(&task.id).unwrap();
        let events = store.events_of(Some(&task.id), &["moved"]).unwrap();
        assert_eq!(events[0].detail["task_updated"], json!(saved.updated));
    }
}
