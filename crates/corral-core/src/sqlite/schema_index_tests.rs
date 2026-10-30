use super::*;

#[test]
fn current_open_children_use_the_combined_partial_index() {
    let mut db = Connection::open_in_memory().unwrap();
    init(&mut db, "fixture").unwrap();
    let mut statement = db
        .prepare(
            "EXPLAIN QUERY PLAN SELECT r.part,r.key,r.current,r.open,r.doc,r.idx FROM task_rows r \
         WHERE r.task=1 AND r.part='plan_review' AND r.current=1 AND r.open=1 ORDER BY r.key ASC",
        )
        .unwrap();
    let details = statement
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("\n");
    assert!(details.contains("task_rows_current_open"), "{details}");
}
