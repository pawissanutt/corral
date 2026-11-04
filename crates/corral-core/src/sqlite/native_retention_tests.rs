use super::*;
use serde_json::json;

#[test]
fn upgrade_backfills_live_counts_turn_order_and_report_tokens_once() {
    let mut db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT);
        CREATE TABLE task_rows(task INTEGER,part TEXT,key TEXT,current INTEGER,open INTEGER,doc TEXT,idx TEXT,
            PRIMARY KEY(task,part,key));
        CREATE TABLE task_row_index(task INTEGER,part TEXT,token TEXT,key TEXT,
            PRIMARY KEY(task,part,token,key));").unwrap();
    let insert = |key: &str, part: &str, doc: Value| {
        db.execute(
            "INSERT INTO task_rows VALUES (1,?1,?2,0,0,?3,'[]')",
            params![part, key, serde_json::to_string(&doc).unwrap()],
        )
        .unwrap();
    };
    let counts = json!(vec!["00000000000000000001"; 10]);
    insert(
        "000001",
        "run",
        json!({"agent_host":{"_records":{"counts":counts,"pending":"00000000000000000000"}}}),
    );
    insert(
        "000001/000000/turns/000001",
        "agent_record",
        json!({
            "generation":1,"seq":9,"origin":{"kind":"bridge"},"ended":"2026-01-01T00:00:00Z"
        }),
    );
    insert(
        "000001/000000/receipts/000001",
        "agent_record",
        json!({
            "version":1,"request_id":"rq-done","op":"done","payload_sha256":"hash",
            "identity":{"store":"fixture","project":"p","role":"worker","task":"T-001",
                "run":"R1","session_id":"session","generation":1,"turn":9},
            "state":"committed","at":"2026-01-01T00:00:00Z"
        }),
    );
    init(&mut db).unwrap();
    let doc: String = db
        .query_row("SELECT doc FROM task_rows WHERE part='run'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let doc: Value = serde_json::from_str(&doc).unwrap();
    assert_eq!(doc["agent_host"]["_records"]["live"], counts);
    let order: String = db
        .query_row(
            "SELECT doc FROM task_rows WHERE key LIKE '%/turn_order/%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&order).unwrap()["key"],
        "000001/000000/turns/000001"
    );
    for token in ["report:done:1:9", "turn:ended"] {
        let count: u32 = db
            .query_row(
                "SELECT count(*) FROM task_row_index WHERE token=?1",
                [token],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
    db.execute("DELETE FROM task_rows WHERE part='agent_record'", [])
        .unwrap();
    init(&mut db).unwrap();
    let count: u32 = db
        .query_row(
            "SELECT count(*) FROM task_rows WHERE part='agent_record'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 0,
        "a reopened store does not rescan retained history"
    );
}
