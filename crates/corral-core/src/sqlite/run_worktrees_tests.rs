use super::*;
use rusqlite::{StatementStatus, params};

fn database() -> Connection {
    let mut db = Connection::open_in_memory().unwrap();
    super::super::schema::init(&mut db, "worktrees-test").unwrap();
    db.execute_batch("INSERT INTO tasks(num,id,rev,title,status,priority,tracker,design,queued,created,updated,spec_rev,spec,state,body,history,doc_version)
        VALUES(1,'T-001',1,'Test','done','P2',0,0,0,'2026-01-01T00:00:00.000000000Z','2026-01-01T00:00:00.000000000Z','','{}','{}','\"\"','{}',1)").unwrap();
    db
}

fn put(db: &Connection, key: &str, current: bool, path: &str) {
    db.execute(
        "INSERT INTO task_rows(task,part,key,current,open,doc) VALUES(1,'run',?1,?2,0,?3)
         ON CONFLICT(task,part,key) DO UPDATE SET current=excluded.current,doc=excluded.doc",
        params![
            key,
            current,
            serde_json::json!({"id": key, "worktree": path, "summary": "x".repeat(16384)})
                .to_string()
        ],
    )
    .unwrap();
}

#[test]
fn scalar_locations_follow_replacements_deletions_and_current_flags() {
    let db = database();
    put(&db, "000001", false, "/old");
    put(&db, "000002", true, "/new");
    let current = read(&db, true).unwrap();
    assert_eq!(
        current,
        vec![RunWorktree {
            task: TaskNum(1),
            run: "000002".into(),
            worktree: "/new".into()
        }]
    );
    assert_eq!(read(&db, false).unwrap().len(), 2);
    put(&db, "000002", false, "/moved");
    put(&db, "000003", true, "/latest");
    assert_eq!(
        read(&db, true).unwrap()[0].worktree,
        std::path::Path::new("/latest")
    );
    assert!(
        read(&db, false)
            .unwrap()
            .iter()
            .any(|r| r.worktree == std::path::Path::new("/moved"))
    );
    db.execute("DELETE FROM task_rows WHERE key='000003'", [])
        .unwrap();
    assert!(read(&db, true).unwrap().is_empty());
}

fn steps(db: &Connection) -> i32 {
    let mut stmt = db.prepare(CURRENT).unwrap();
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(2))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(rows, ["/current"]);
    stmt.get_status(StatementStatus::VmStep)
}

#[test]
fn current_lookup_is_covering_and_independent_of_old_history() {
    let db = database();
    put(&db, "999999", true, "/current");
    for i in 0..10 {
        put(&db, &format!("{i:06}"), false, "/old");
    }
    let small = steps(&db);
    for i in 10..100 {
        put(&db, &format!("{i:06}"), false, "/old");
    }
    assert_eq!(small, steps(&db));
    for sql in [ALL, CURRENT] {
        let mut stmt = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let plan = stmt
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n");
        assert!(plan.contains("task_rows_worktrees"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
        // A covering expression index reads no JSON and executes no JSON functions.
        let mut stmt = db.prepare(&format!("EXPLAIN {sql}")).unwrap();
        let ops = stmt
            .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(!ops.iter().any(|(op, _)| op == "Function"), "{ops:?}");
        let table = ops.iter().find(|(op, _)| op == "OpenRead").unwrap().1;
        assert!(
            !ops.iter()
                .any(|(op, cursor)| op == "Column" && *cursor == table),
            "{ops:?}"
        );
    }
}

#[test]
fn additive_index_is_backfilled_for_existing_databases() {
    let mut db = database();
    db.execute_batch("DROP INDEX task_rows_worktrees").unwrap();
    put(&db, "000001", true, "/existing");
    super::super::schema::init(&mut db, "ignored").unwrap();
    assert_eq!(
        read(&db, true).unwrap()[0].worktree,
        std::path::Path::new("/existing")
    );
}
