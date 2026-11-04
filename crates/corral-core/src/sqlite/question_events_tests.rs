use super::*;
use crate::{Status, Store};
use rusqlite::{Connection, params};
use serde_json::json;

#[test]
fn existing_database_backfills_only_unlogged_transitions_once() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let task = store
        .create("Question migration", None, Status::Running, "")
        .unwrap();
    let at = crate::backend::conformance::epoch();
    let question = HistoryRow {
        part: TaskPart::AgentRecord,
        key: "000001/questions/000001".into(),
        current: false,
        open: true,
        index: vec![],
        doc: json!({"version":1,"key":"T-001/R1/Q1","revision":1,"kind":"clarification",
            "asked":{"request_id":"ask","payload_sha256":"00","session_id":"session","generation":1,"at":at},
            "blocking":true,"text":"question","state":"escalated",
            "wake":{"route":"none","key":"wake","state":"pending"},
            "triage":[{"action":"escalate","revision":1,"by":"pm","at":at}]}),
    };
    let transitions = crate::backend::question_events::transitions(&question).unwrap();
    store
        .log_event(Some(&task.id), &transitions[0].0, transitions[0].1.clone())
        .unwrap();
    let path = super::super::SqliteBackend::db_path(store.dir());
    drop(store);
    let mut db = Connection::open(path).unwrap();
    // Simulate the pre-outbox schema's canonical question and event, with no derived rows.
    db.execute("DELETE FROM records WHERE area = 'question-event-acks'", [])
        .unwrap();
    db.execute(
        "DELETE FROM meta WHERE key = 'question_event_outbox_v1'",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO task_rows(task,part,key,current,open,doc,idx) VALUES (1,?1,?2,0,1,?3,'[]')",
        params![
            name(&question.part).unwrap(),
            question.key,
            question.doc.to_string()
        ],
    )
    .unwrap();
    super::super::schema::init(&mut db, "fixture").unwrap();
    let count = |db: &Connection, area: &str| -> i64 {
        db.query_row(
            "SELECT COUNT(*) FROM records WHERE area = ?1",
            [area],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(count(&db, crate::backend::question_events::ACKS), 1);
    assert_eq!(
        count(&db, &crate::backend::question_events::area(&task.id)),
        1
    );
    // A completed migration never scans again, even if derived work is later consumed.
    db.execute(
        "DELETE FROM records WHERE area LIKE 'question-event-work/%'",
        [],
    )
    .unwrap();
    super::super::schema::init(&mut db, "fixture").unwrap();
    assert_eq!(
        count(&db, &crate::backend::question_events::area(&task.id)),
        0
    );
}
