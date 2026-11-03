//! Count actual SQLite VM work, not just decoded output: scans can decode zero rows.
use super::*;
use rusqlite::params;

fn fixture(history: i64) -> SqliteView {
    let mut db = Connection::open_in_memory().unwrap();
    super::super::schema::init(&mut db, "hot-reads").unwrap();
    for n in 1..=history + 1 {
        db.execute(
            "INSERT INTO tasks(num,id,rev,title,status,priority,tracker,design,queued,created,updated,
             spec_rev,spec,state,body,history,doc_version) VALUES
             (?1,?2,1,'task',?3,'normal',0,0,0,?4,?4,'rev','','{}','','{}',1)",
            params![n, format!("T-{n:03}"), if n == 1 { "ready" } else { "done" }, ts(Timestamp::UNIX_EPOCH)],
        ).unwrap();
    }
    for n in 0..history {
        db.execute(
            "INSERT INTO task_rows(task,part,key,current,open,doc,idx)
             VALUES (1,'artifact',?1,0,0,'{}','[]')",
            [format!("{n:06}")],
        )
        .unwrap();
        db.execute(
            "INSERT INTO task_rows(task,part,key,current,open,doc,idx)
             VALUES (1,'gate_attempt',?1,1,0,'{}','[]')",
            [format!("{n:06}")],
        )
        .unwrap();
    }
    db.execute(
        "INSERT INTO task_row_index(task,part,token,key) VALUES (1,'artifact','hit','000000')",
        [],
    )
    .unwrap();
    SqliteView::open(
        db,
        Arc::default(),
        Arc::new(SystemClock),
        Duration::from_secs(60),
        None,
    )
    .unwrap()
}

fn cost(read: impl FnOnce()) -> i32 {
    load::VM_STEPS.with(|steps| steps.set(0));
    read();
    load::VM_STEPS.with(|steps| steps.get())
}

#[test]
fn hot_queries_do_not_scan_terminal_tasks_or_unmatched_history() {
    let costs = [2, 8].map(|history| {
        let view = fixture(history);
        let board = cost(|| assert_eq!(view.board(None).unwrap().heads.len(), 1));
        let heads = cost(|| {
            assert_eq!(
                view.heads(&HeadQuery::Open { project: None }, PageReq::first(10))
                    .unwrap()
                    .items
                    .len(),
                1
            )
        });
        let tokens = ["miss", "hit"].map(|token| {
            cost(|| {
                let page = view
                    .rows(
                        TaskNum(1),
                        &RowQuery::part(TaskPart::Artifact).with_index(token),
                        PageReq::first(1),
                    )
                    .unwrap();
                assert_eq!(page.items.len(), usize::from(token == "hit"));
            })
        });
        let current = cost(|| {
            assert!(
                view.rows(
                    TaskNum(1),
                    &RowQuery::part(TaskPart::Run).current(),
                    PageReq::first(1)
                )
                .unwrap()
                .items
                .is_empty()
            )
        });
        [board, heads, tokens[0], tokens[1], current]
    });
    println!("SQLite VM steps (2 vs 8 history rows): {costs:?}");
    assert_eq!(
        costs[0], costs[1],
        "history-sized SQL work despite bounded decoded output"
    );
}
