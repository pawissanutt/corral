//! The lifecycle reads stay bounded by their page (T-334 review F6): a first page of the due and
//! policy reads costs about the same number of SQLite VM steps over a backlog of 17 rows as over
//! 1,000, because each is one index range read in index order with no sort before the limit.

use jiff::{SignedDuration, Timestamp};
use rusqlite::{Connection, StatementStatus};

use super::{LIFECYCLE_ABOVE, LIFECYCLE_BELOW, LIFECYCLE_DUE};
use crate::backend::{LifecycleOutcome, LifecycleWrite, Operation, StoreBackend};
use crate::sqlite::{SqliteBackend, SqliteOptions};

const PAGE: &str = "LIMIT 17 OFFSET 0";

fn t0() -> Timestamp {
    "2026-10-29T00:00:00Z".parse().unwrap()
}

/// A store with `rows` deferred rows due at increasing times under policy digest `old`.
fn backlog(rows: usize) -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let backend = SqliteBackend::open(dir.path(), SqliteOptions::default()).unwrap();
    for n in 0..rows {
        let at = t0()
            .checked_add(SignedDuration::from_secs(n as i64))
            .unwrap();
        let write = LifecycleWrite {
            task: format!("T-{n:04}"),
            generation: "R1".into(),
            target: "worktree".into(),
            expected: None,
            outcome: LifecycleOutcome::Deferred,
            policy_digest: "old".into(),
            reason: None,
            bytes: None,
            next_check_at: Some(at),
            at,
        };
        backend.apply(Operation::Lifecycle(write)).unwrap();
    }
    let conn = Connection::open(SqliteBackend::db_path(dir.path())).unwrap();
    (dir, conn)
}

/// The VM steps and rows of one first page of `tail` with `args`.
fn page_cost(conn: &Connection, tail: &str, args: &[&dyn rusqlite::ToSql]) -> (i32, usize) {
    let sql = format!("SELECT task FROM lifecycle {tail} {PAGE}");
    let mut statement = conn.prepare(&sql).unwrap();
    let rows = statement
        .query_map(args, |row| row.get::<_, String>(0))
        .unwrap()
        .count();
    (statement.get_status(StatementStatus::VmStep), rows)
}

#[test]
fn due_and_policy_pages_cost_the_same_over_a_small_and_a_large_backlog() {
    let far = super::ts(t0().checked_add(SignedDuration::from_hours(24)).unwrap());
    let reads: [(&str, Vec<Box<dyn rusqlite::ToSql>>); 3] = [
        (LIFECYCLE_DUE, vec![Box::new("deferred"), Box::new(far)]),
        (LIFECYCLE_BELOW, vec![Box::new("pnew")]),
        (LIFECYCLE_ABOVE, vec![Box::new("a")]),
    ];
    let (_small_dir, small) = backlog(17);
    let (_large_dir, large) = backlog(1000);
    for (tail, args) in &reads {
        let args: Vec<&dyn rusqlite::ToSql> = args.iter().map(|arg| arg.as_ref()).collect();
        let (small_steps, small_rows) = page_cost(&small, tail, &args);
        let (large_steps, large_rows) = page_cost(&large, tail, &args);
        assert_eq!((small_rows, large_rows), (17, 17), "{tail}");
        assert!(
            large_steps <= small_steps + small_steps / 4,
            "{tail}: {small_steps} steps over 17 rows, {large_steps} over 1000"
        );
    }
}
