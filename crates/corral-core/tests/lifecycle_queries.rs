//! The indexed lifecycle reads the lifecycle-gc sync pass relies on (T-334): deferred rows due
//! by a time, rows recorded below or above a policy digest, and one row by key, on the fake and
//! on SQLite.

use anyhow::Result;
use corral_core::backend::conformance::fake_fixture;
use corral_core::backend::{
    LifecycleOutcome, LifecycleWrite, Operation, PageReq, StorageQuery, StorageRow, StoreBackend,
};
use corral_core::sqlite::{SqliteBackend, SqliteOptions};
use jiff::{SignedDuration, Timestamp};

fn t0() -> Timestamp {
    "2026-10-29T00:00:00Z".parse().unwrap()
}

fn at(secs: i64) -> Timestamp {
    t0().checked_add(SignedDuration::from_secs(secs)).unwrap()
}

fn put(
    backend: &dyn StoreBackend,
    task: &str,
    outcome: LifecycleOutcome,
    digest: &str,
    next: Option<i64>,
) {
    backend
        .apply(Operation::Lifecycle(LifecycleWrite {
            task: task.into(),
            generation: "R1".into(),
            target: "worktree".into(),
            expected: None,
            outcome,
            policy_digest: digest.into(),
            reason: None,
            bytes: None,
            next_check_at: next.map(at),
            at: t0(),
        }))
        .unwrap();
}

fn tasks(backend: &dyn StoreBackend, query: &StorageQuery, req: PageReq) -> (Vec<String>, bool) {
    let page = backend.view().unwrap().storage_rows(query, req).unwrap();
    let tasks = page
        .items
        .into_iter()
        .map(|row| match row {
            StorageRow::Lifecycle(row) => row.task,
            other => panic!("not a lifecycle row: {other:?}"),
        })
        .collect();
    (tasks, page.next.is_some())
}

fn exercise(backend: &dyn StoreBackend) {
    put(backend, "T-1", LifecycleOutcome::Deferred, "p1", Some(60));
    put(backend, "T-2", LifecycleOutcome::Deferred, "p1", Some(600));
    put(backend, "T-3", LifecycleOutcome::Retained, "p0", None);
    put(backend, "T-4", LifecycleOutcome::Deferred, "p0", Some(30));
    put(backend, "T-5", LifecycleOutcome::Reclaimed, "p2", None);

    let due = StorageQuery::LifecycleDue {
        outcome: LifecycleOutcome::Deferred,
        now: at(120),
    };
    assert_eq!(
        tasks(backend, &due, PageReq::first(10)),
        (vec!["T-4".into(), "T-1".into()], false),
        "due rows, earliest check first; not yet due and retained rows excluded"
    );
    let (first, more) = tasks(backend, &due, PageReq::first(1));
    assert_eq!(
        (first, more),
        (vec!["T-4".to_owned()], true),
        "a bounded batch"
    );
    let exact = StorageQuery::LifecycleDue {
        outcome: LifecycleOutcome::Deferred,
        now: at(60),
    };
    assert_eq!(
        tasks(backend, &exact, PageReq::first(10)).0,
        vec!["T-4", "T-1"],
        "a row due exactly now is due"
    );

    let below = StorageQuery::LifecyclePolicyBelow {
        digest: "p1".into(),
    };
    assert_eq!(
        tasks(backend, &below, PageReq::first(10)),
        (vec!["T-3".into(), "T-4".into()], false),
        "the range below the digest, in index order"
    );
    let above = StorageQuery::LifecyclePolicyAbove {
        digest: "p1".into(),
    };
    assert_eq!(tasks(backend, &above, PageReq::first(10)).0, vec!["T-5"]);
    let (first, more) = tasks(backend, &below, PageReq::first(1));
    assert_eq!(
        (first, more),
        (vec!["T-3".to_owned()], true),
        "a bounded batch"
    );

    let key = |task: &str, generation: &str| StorageQuery::LifecycleKey {
        task: task.into(),
        generation: generation.into(),
        target: "worktree".into(),
    };
    assert_eq!(
        tasks(backend, &key("T-2", "R1"), PageReq::first(5)).0,
        vec!["T-2"]
    );
    assert!(
        tasks(backend, &key("T-2", "R2"), PageReq::first(5))
            .0
            .is_empty()
    );
}

#[test]
fn fake_backend_serves_due_and_policy_reads() {
    let fixture = fake_fixture();
    exercise(fixture.backend.as_ref());
}

#[test]
fn sqlite_backend_serves_due_and_policy_reads() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let backend = SqliteBackend::open(dir.path(), SqliteOptions::default())?;
    exercise(&backend);
    Ok(())
}
