//! The task store on the database (T-320 store-tasks-db): init and open protocol, the store gate,
//! row-level writes, one-view snapshots, concurrency through the leases and the CAS precondition.

mod store_support;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

use corral_core::backend::{
    ApplyError, ChangeScope, HistoryRow, Operation, StoreBackend, TaskChange, TaskNum, TaskPart,
    TaskParts, UpdateForm, UpdateTask,
};
use corral_core::sqlite::{SqliteBackend, SqliteOptions};
use corral_core::{HistorySummary, Status, Store};
use store_support::*;

const CHILD: &str = "CORRAL_STORE_TASKS_CHILD";
const ROOT: &str = "CORRAL_STORE_TASKS_ROOT";

fn rows(store: &Store, id: &str) -> BTreeMap<(TaskPart, String), HistoryRow> {
    let num = TaskNum(id.trim_start_matches("T-").parse().unwrap());
    store
        .view()
        .unwrap()
        .task(num, &TaskParts::All)
        .unwrap()
        .unwrap()
        .rows
        .into_iter()
        .map(|row| ((row.part, row.key.clone()), row))
        .collect()
}

fn changed_rows(
    before: &BTreeMap<(TaskPart, String), HistoryRow>,
    after: &BTreeMap<(TaskPart, String), HistoryRow>,
) -> Vec<(TaskPart, String)> {
    let mut keys: Vec<_> = after
        .iter()
        .filter(|(key, row)| before.get(*key) != Some(*row))
        .map(|(key, _)| key.clone())
        .collect();
    keys.extend(before.keys().filter(|k| !after.contains_key(*k)).cloned());
    keys
}

fn summary(store: &Store, id: &str) -> HistorySummary {
    let num = TaskNum(id.trim_start_matches("T-").parse().unwrap());
    serde_json::from_value(store.view().unwrap().head(num).unwrap().unwrap().summary).unwrap()
}

#[test]
fn init_creates_the_database_with_origin_init_and_fence_four() {
    let (dir, store) = store();
    let db = dir.path().join(".corral/db/corral.db");
    assert!(db.is_file());
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".corral/state/schema-version")).unwrap(),
        "4\n"
    );
    let backend =
        SqliteBackend::open(&dir.path().join(".corral"), SqliteOptions::default()).unwrap();
    assert_eq!(backend.meta("origin").unwrap().as_deref(), Some("init"));
    assert!(!dir.path().join(".corral/tasks").exists());
    let task = store
        .create("First", None, Status::Backlog, "body\n")
        .unwrap();
    assert_eq!(
        Store::open(dir.path()).unwrap().get(&task.id).unwrap(),
        task
    );
    // A second init keeps the store as it is.
    assert_eq!(
        Store::init(dir.path()).unwrap().get(&task.id).unwrap(),
        task
    );
}

#[test]
fn rich_tasks_round_trip_through_rows_and_fresh_processes() {
    let (dir, store) = store();
    let task = rich(&store, 3, 4, 3, 3);
    assert_eq!(store.get(&task.id).unwrap(), task);
    let reopened = Store::open(dir.path()).unwrap();
    assert_eq!(reopened.get(&task.id).unwrap(), task);
    assert_eq!(reopened.list().unwrap(), vec![task.clone()]);
    assert_eq!(
        summary(&store, &task.id),
        HistorySummary::of(&task).unwrap()
    );
}

#[test]
fn a_full_update_writes_only_the_changed_run_row_and_a_no_op_writes_nothing() {
    // Given a task with many runs.
    let (_dir, store) = store();
    let task = rich(&store, 6, 2, 2, 1);
    let before = rows(&store, &task.id);
    let seq = store.view().unwrap().seq();
    // When one field of the third run changes.
    let saved = store
        .update(&task.id, |t| {
            t.runs[2].summary = Some("third run summary".into());
            Ok(())
        })
        .unwrap();
    // Then only that run row and the summary are written.
    let after = rows(&store, &task.id);
    assert_eq!(
        changed_rows(&before, &after),
        [(TaskPart::Run, "000003".to_owned())]
    );
    let page = store.changes_since(seq, 10).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(
        page.entries[0].scopes,
        [ChangeScope::Task {
            num: TaskNum(1),
            parts: vec![TaskPart::Run]
        }]
    );
    assert_eq!(
        summary(&store, &task.id),
        HistorySummary::of(&saved).unwrap()
    );
    assert_eq!(store.get(&task.id).unwrap(), saved);
    // When a callback changes nothing, no row and no change entry are written.
    let seq = store.view().unwrap().seq();
    let unchanged = store.update(&task.id, |_| Ok(())).unwrap();
    assert_eq!(unchanged, saved);
    assert_eq!(store.view().unwrap().seq(), seq);
    assert_eq!(rows(&store, &task.id), after);
    assert_eq!(store.get(&task.id).unwrap(), saved);
}

#[test]
fn editing_one_review_child_writes_that_child_row_and_its_container_facts() {
    // Given a task whose older run and current run each have ten review children.
    let (_dir, store) = store();
    let task = rich(&store, 2, 10, 1, 1);
    store
        .update(&task.id, |t| {
            let id = t.id.clone();
            t.runs[0].review_runs = (1..=10)
                .map(|n| {
                    serde_json::from_value(review_child(&id, "R0001", n, 1, true, "approve"))
                        .unwrap()
                })
                .collect();
            Ok(())
        })
        .unwrap();
    let task = store.get(&task.id).unwrap();
    for (run, child) in [(0, 6), (1, 9)] {
        let before = rows(&store, &task.id);
        // When one field of that child changes.
        let saved = store
            .update(&task.id, |t| {
                t.runs[run].review_runs[child].summary = Some(format!("edited {run}/{child}"));
                Ok(())
            })
            .unwrap();
        // Then exactly that child row and its run's facts are written.
        let after = rows(&store, &task.id);
        let seq = format!("{:06}", run + 1);
        assert_eq!(
            changed_rows(&before, &after),
            [
                (TaskPart::Run, seq.clone()),
                (TaskPart::ReviewRun, format!("{seq}/{:06}", child + 1)),
            ]
        );
        let children_rev = |rows: &BTreeMap<(TaskPart, String), HistoryRow>| -> u64 {
            let facts = &rows[&(TaskPart::Run, seq.clone())].doc["_facts"];
            facts["children_rev"].as_str().unwrap().parse().unwrap()
        };
        assert_eq!(children_rev(&after), children_rev(&before) + 1);
        assert_eq!(store.get(&task.id).unwrap(), saved);
        let export = Store::render_task_markdown(&store.get(&task.id).unwrap());
        assert_eq!(
            Store::parse_task_markdown(&export).unwrap().runs,
            saved.runs
        );
    }
}

fn child(role: &str, root: &std::path::Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "child_process",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, role)
        .env(ROOT, root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    command
}

fn hear(child: &mut Child, expected: &str) {
    let out = child.stdout.as_mut().unwrap();
    let mut reader = BufReader::new(out);
    let mut line = String::new();
    loop {
        line.clear();
        assert_ne!(
            reader.read_line(&mut line).unwrap(),
            0,
            "child exited before {expected}"
        );
        if line.trim_end().ends_with(expected) {
            return;
        }
    }
}

fn finish(mut child: Child) {
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
}

/// Re-executed by the tests below to act as another process.
#[test]
fn child_process() {
    let (Ok(role), Ok(root)) = (std::env::var(CHILD), std::env::var(ROOT)) else {
        return;
    };
    let root = std::path::Path::new(&root);
    let store = Store::open(root).unwrap();
    match role.as_str() {
        "hold" => {
            println!("held");
            std::io::stdout().flush().unwrap();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).unwrap();
        }
        "rename-b" => {
            store
                .update("T-002", |t| {
                    t.title = "B after".into();
                    Ok(())
                })
                .unwrap();
            println!("renamed");
        }
        role if role.starts_with("increment:") => {
            let mut parts = role.trim_start_matches("increment:").split(':');
            let id = parts.next().unwrap();
            let times: usize = parts.next().unwrap().parse().unwrap();
            let head = parts.next() == Some("head");
            for _ in 0..times {
                if head {
                    store
                        .update_head(id, |h| {
                            h.rounds += 1;
                            Ok(())
                        })
                        .unwrap();
                } else {
                    store
                        .update(id, |t| {
                            t.rounds += 1;
                            Ok(())
                        })
                        .unwrap();
                }
            }
        }
        other => panic!("unknown child role {other}"),
    }
}

#[test]
fn a_snapshot_reads_every_task_and_head_on_one_view() {
    // Given tasks A and B and a snapshot that has read A.
    let (dir, store) = store();
    let a = store.create("A", None, Status::Backlog, "").unwrap();
    let b = store.create("B before", None, Status::Backlog, "").unwrap();
    let snapshot = store.read_only_snapshot().unwrap();
    assert_eq!(snapshot.get(&a.id).unwrap(), a);
    // When another process commits a change to B.
    let mut other = child("rename-b", dir.path()).spawn().unwrap();
    hear(&mut other, "renamed");
    finish(other);
    // Then the snapshot still reads B's pre-commit state, as a task and as a head.
    assert_eq!(snapshot.get(&b.id).unwrap().title, "B before");
    assert_eq!(snapshot.head(&b.id).unwrap().title, "B before");
    // And a new snapshot sees the commit.
    let fresh = store.read_only_snapshot().unwrap();
    assert_eq!(fresh.get(&b.id).unwrap().title, "B after");
    assert_eq!(fresh.head(&b.id).unwrap().title, "B after");
}

#[test]
fn updates_from_eight_processes_lose_nothing() {
    for head in [false, true] {
        // Given one task and eight tasks.
        let (dir, store) = store();
        let ids: Vec<String> = (0..8)
            .map(|n| {
                store
                    .create(&format!("Counter {n}"), None, Status::Backlog, "")
                    .unwrap()
                    .id
            })
            .collect();
        for targets in [vec![ids[0].clone(); 8], ids.clone()] {
            let before: Vec<u32> = ids.iter().map(|id| store.get(id).unwrap().rounds).collect();
            // When eight processes each increment their target five times.
            let children: Vec<Child> = targets
                .iter()
                .map(|id| {
                    let role = format!("increment:{id}:5{}", if head { ":head" } else { "" });
                    child(&role, dir.path()).spawn().unwrap()
                })
                .collect();
            for child in children {
                finish(child);
            }
            // Then every increment survived.
            for (i, id) in ids.iter().enumerate() {
                let expected = before[i] + 5 * targets.iter().filter(|t| *t == id).count() as u32;
                assert_eq!(store.get(id).unwrap().rounds, expected, "{id} head={head}");
            }
        }
    }
}

#[test]
fn a_writer_that_bypasses_the_leases_gets_a_conflict() {
    // Given a stale revision read by a writer that holds no lease.
    let (dir, store) = store();
    let task = store.create("Raced", None, Status::Backlog, "").unwrap();
    let bypass =
        SqliteBackend::open(&dir.path().join(".corral"), SqliteOptions::default()).unwrap();
    let stale = bypass.view().unwrap().head(TaskNum(1)).unwrap().unwrap();
    store
        .update(&task.id, |t| {
            t.title = "Leased writer".into();
            Ok(())
        })
        .unwrap();
    // When it writes from the stale revision.
    let result = bypass.apply(Operation::UpdateTask(UpdateTask {
        change: TaskChange {
            num: TaskNum(1),
            expected_rev: stale.header.rev,
            fields: stale.header.fields.clone(),
            doc: stale.doc.clone(),
            summary: stale.summary.clone(),
            rows: Vec::new(),
            events: Vec::new(),
            records: Vec::new(),
            aggregates: Vec::new(),
            reservations: Vec::new(),
            conditions: Vec::new(),
        },
        form: UpdateForm::Head,
    }));
    // Then it is refused, never a silent overwrite.
    assert!(matches!(result, Err(ApplyError::Conflict(_))), "{result:?}");
    assert_eq!(store.get(&task.id).unwrap().title, "Leased writer");
}

#[test]
fn open_accepts_only_published_finalizing_finalized_or_init() {
    let (dir, store) = store();
    drop(store);
    let corral = dir.path().join(".corral");
    let journal = corral.join("migration.json");
    for (state, step, accepted, hint) in [
        ("fenced", None, false, "migrate --apply"),
        ("importing", None, false, "migrate --apply"),
        ("verified", None, false, "migrate --rollback"),
        ("published", None, true, ""),
        ("finalizing", None, true, ""),
        ("finalized", None, true, ""),
        ("rolling_back", Some("intent"), false, "migrate --rollback"),
        (
            "rolling_back",
            Some("archived"),
            false,
            "migrate --rollback",
        ),
        (
            "rolling_back",
            Some("quarantined"),
            false,
            "migrate --rollback",
        ),
        ("rolling_back", Some("cleaned"), false, "migrate --rollback"),
        (
            "rolling_back",
            Some("fence_restored"),
            false,
            "migrate --rollback",
        ),
    ] {
        let mut doc = serde_json::json!({"state": state});
        if let Some(step) = step {
            doc["step"] = step.into();
        }
        std::fs::write(&journal, doc.to_string()).unwrap();
        let result = Store::open(dir.path());
        assert_eq!(result.is_ok(), accepted, "{state} {step:?}");
        if let Err(error) = result {
            let message = format!("{error:#}");
            assert!(
                message.contains(state) || message.contains(step.unwrap_or("-")),
                "{message}"
            );
            assert!(message.contains(hint), "{message}");
        }
    }
    std::fs::remove_file(&journal).unwrap();
    assert!(
        Store::open(dir.path()).is_ok(),
        "no journal and origin init"
    );
    std::fs::write(corral.join("state/schema-version"), "3\n").unwrap();
    let legacy = format!("{:#}", Store::open(dir.path()).unwrap_err());
    assert!(legacy.contains("corral migrate"), "{legacy}");
}

#[test]
fn every_open_holds_the_store_gate_shared_until_its_last_connection_closes() {
    // Given two processes holding the store open.
    let (dir, store) = store();
    let corral = dir.path().join(".corral");
    drop(store);
    let mut holders: Vec<Child> = (0..2)
        .map(|_| child("hold", dir.path()).spawn().unwrap())
        .collect();
    for holder in &mut holders {
        hear(holder, "held");
    }
    // Then each registered, and maintenance cannot take the gate exclusively.
    let registered = corral_core::store_holders(&corral).unwrap();
    for holder in &holders {
        assert!(
            registered
                .iter()
                .any(|h| h.starts_with(&format!("{}-", holder.id()))),
            "{registered:?}"
        );
    }
    assert!(
        corral_core::try_exclusive_store_gate(&corral)
            .unwrap()
            .is_none()
    );
    let first = holders.remove(0);
    finish(first);
    assert!(
        corral_core::try_exclusive_store_gate(&corral)
            .unwrap()
            .is_none()
    );
    // When both exit, the exclusive gate is free and no holder remains.
    finish(holders.remove(0));
    let gate = corral_core::try_exclusive_store_gate(&corral).unwrap();
    assert!(gate.is_some());
    assert!(corral_core::store_holders(&corral).unwrap().is_empty());
    // And while maintenance holds it, no store opens.
    let refused = format!("{:#}", Store::open(dir.path()).unwrap_err());
    assert!(refused.contains("maintenance"), "{refused}");
    drop(gate);
    assert!(Store::open(dir.path()).is_ok());
}

#[test]
fn creation_allocates_ids_without_a_next_id_lock() {
    let (dir, store) = store();
    let ids: Vec<String> = (0..3)
        .map(|n| {
            store
                .create(&format!("T{n}"), None, Status::Backlog, "")
                .unwrap()
                .id
        })
        .collect();
    assert_eq!(ids, ["T-001", "T-002", "T-003"]);
    assert!(!dir.path().join(".corral/lock/next-id.lock").exists());
}
