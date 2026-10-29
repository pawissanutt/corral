//! Opt-in checks on a disposable copy of a real legacy board (the T-253 prepared fixture), run
//! by the migration rehearsal:
//! `CORRAL_MIGRATE_FIXTURE=<copy root> cargo test -p corral-core --test migrate_fixture -- --ignored`.
//! The copy is migrated in place. Every task's rows must equal an ordinary v2 write of its
//! legacy record (gate attempts and event-only panes aside), every run's events must be
//! readable by run key, and every gate receipt must stay exactly as valid as before migration.

#[path = "migrate_support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use corral_core::backend::{
    EventQuery, HistoryRow, PageReq, StoreBackend, TaskNum, TaskPart, TaskParts,
};
use corral_core::{GateResult, Store, Task};
use sha2::{Digest, Sha256};

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn legacy_tasks(corral: &std::path::Path) -> Vec<Task> {
    let mut tasks = Vec::new();
    for entry in fs::read_dir(corral.join("tasks")).unwrap() {
        let path = entry.unwrap().path();
        let text = fs::read_to_string(&path).unwrap();
        tasks.push(corral_core::parse_task_record(&text, &path).unwrap().task);
    }
    tasks.sort_by_key(|t| corral_core::Store::normalize_id(&t.id).unwrap());
    tasks
}

/// `(task, run)` -> the number of legacy events naming that run.
fn legacy_run_events(corral: &std::path::Path) -> BTreeMap<(String, String), usize> {
    let mut out = BTreeMap::new();
    for line in fs::read_to_string(corral.join("events.jsonl"))
        .unwrap()
        .lines()
    {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let (Some(task), Some(run)) = (event["task"].as_str(), event["detail"]["run"].as_str()) {
            *out.entry((task.to_owned(), run.to_owned())).or_default() += 1;
        }
    }
    out
}

fn rows(
    corral: &std::path::Path,
    num: u64,
) -> (serde_json::Value, serde_json::Value, Vec<HistoryRow>) {
    let backend = corral_core::sqlite::SqliteBackend::open(
        corral,
        corral_core::sqlite::SqliteOptions::default(),
    )
    .unwrap();
    let doc = backend
        .view()
        .unwrap()
        .task(TaskNum(num), &TaskParts::All)
        .unwrap()
        .unwrap();
    let mut fields = serde_json::to_value(&doc.head.header.fields).unwrap();
    fields.as_object_mut().unwrap().remove("updated");
    let mut rows = doc.rows;
    rows.sort_by(|a, b| (a.part, &a.key).cmp(&(b.part, &b.key)));
    (fields, doc.head.summary, rows)
}

#[test]
#[ignore = "needs CORRAL_MIGRATE_FIXTURE, a disposable copy of a legacy board"]
fn a_real_board_migrates_to_ordinary_rows_run_keyed_events_and_valid_receipts() {
    let root = PathBuf::from(std::env::var("CORRAL_MIGRATE_FIXTURE").unwrap());
    let corral = root.join(".corral");
    let tasks = legacy_tasks(&corral);
    let run_events = legacy_run_events(&corral);
    // Receipt validity before migration, from the legacy gate files.
    let mut started: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
    for line in fs::read_to_string(corral.join("events.jsonl"))
        .unwrap()
        .lines()
    {
        let Ok(e) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if e["kind"] != "gate_started" {
            continue;
        }
        let (Some(task), Some(run), Some(attempt)) = (
            e["task"].as_str(),
            e["detail"]["run"].as_str(),
            e["detail"]["identity"]["attempt"].as_str(),
        ) else {
            continue;
        };
        if let Ok(commands) = serde_json::from_value(e["detail"]["commands"].clone()) {
            started.insert(
                (task.to_owned(), run.to_owned(), attempt.to_owned()),
                commands,
            );
        }
    }
    let mut before = BTreeMap::new();
    for task in &tasks {
        for run in &task.runs {
            let Some(gate) = &run.gate else { continue };
            let Some(identity) = &gate.identity else {
                continue;
            };
            if gate.result == GateResult::Running {
                continue;
            }
            let stem = format!("state/gates/{}-{}-{}", task.id, run.id, identity.attempt);
            let valid = fs::read(corral.join(format!("{stem}.log")))
                .ok()
                .is_some_and(|log| {
                    let key = (task.id.clone(), run.id.clone(), identity.attempt.clone());
                    let commands = started.get(&key).cloned().unwrap_or_default();
                    gate.log_sha256.as_deref() == Some(sha(&log).as_str())
                        && support::command_log_valid(&log, &commands, &gate.commands, gate.result)
                });
            before.insert(
                (task.id.clone(), run.id.clone(), identity.attempt.clone()),
                valid,
            );
        }
    }

    // When the board is migrated and the same records are saved by an ordinary v2 write.
    corral_core::migrate::Migrate::new(&root).apply().unwrap();
    let fresh = tempfile::tempdir().unwrap();
    let store = Store::init(fresh.path()).unwrap();
    for task in &tasks {
        store.save(task).unwrap();
    }
    drop(store);

    // Then every task's header fields, summary and derived rows equal the ordinary write's,
    // and the migration adds only gate attempts and event-only panes.
    let mut parts: BTreeMap<String, usize> = BTreeMap::new();
    let mut extra_panes = 0;
    for task in &tasks {
        let num = corral_core::Store::normalize_id(&task.id)
            .unwrap()
            .trim_start_matches("T-")
            .parse()
            .unwrap();
        let (fields, summary, migrated) = rows(&corral, num);
        let (written_fields, written_summary, written) = rows(&fresh.path().join(".corral"), num);
        assert_eq!(fields, written_fields, "{} header fields", task.id);
        assert_eq!(summary, written_summary, "{} summary", task.id);
        let derived = |rows: &[HistoryRow]| -> Vec<HistoryRow> {
            rows.iter()
                .filter(|r| !matches!(r.part, TaskPart::CleanupPane | TaskPart::GateAttempt))
                .cloned()
                .collect()
        };
        assert_eq!(derived(&migrated), derived(&written), "{} rows", task.id);
        for row in &migrated {
            *parts.entry(format!("{:?}", row.part)).or_default() += 1;
        }
        for row in migrated
            .iter()
            .filter(|r| r.part == TaskPart::CleanupPane && !written.contains(r))
        {
            let kind = row.doc["kind"].as_str().unwrap_or_default();
            assert!(
                matches!(kind, "gate" | "review" | "workspace"),
                "{} unexpected cleanup row {}",
                task.id,
                row.key
            );
            extra_panes += 1;
        }
    }
    println!(
        "tasks {}; rows by part {parts:?}; event-only panes {extra_panes}",
        tasks.len()
    );

    // And every run's legacy events are read back by their run key.
    let backend = corral_core::sqlite::SqliteBackend::open(
        &corral,
        corral_core::sqlite::SqliteOptions::default(),
    )
    .unwrap();
    let view = backend.view().unwrap();
    for ((task, run), expected) in &run_events {
        let query = EventQuery {
            after: None,
            task: Some(task.clone()),
            kinds: Vec::new(),
            run: Some(run.clone()),
            newest_first: false,
            valid_only: false,
        };
        let mut found = 0;
        let mut page = PageReq::first(500);
        loop {
            let result = view.events(&query, page).unwrap();
            found += result.items.len();
            match result.next {
                Some(next) => {
                    page = PageReq {
                        after: Some(next),
                        limit: 500,
                    }
                }
                None => break,
            }
        }
        assert_eq!(found, *expected, "{task} {run} events by run key");
    }
    println!("run-keyed event reads {}", run_events.len());
    drop(view);
    drop(backend);

    // And every collected gate receipt is exactly as valid through its keyed row as it was
    // through its legacy files.
    let store = Store::open(&root).unwrap();
    let mut valid = 0;
    for ((task, run, attempt), was) in &before {
        let row = store.gate_attempt(task, run, attempt).unwrap().unwrap();
        let record = store.get(task).unwrap();
        let gate = record
            .runs
            .iter()
            .find(|r| &r.id == run)
            .and_then(|r| r.gate.clone())
            .unwrap();
        let log = row.log.as_ref().and_then(|l| store.blob_bytes(l).unwrap());
        let commands: Vec<String> =
            serde_json::from_slice(&store.blob_bytes(&row.commands).unwrap().unwrap()).unwrap();
        let now = log.is_some_and(|log| {
            gate.log_sha256.as_deref() == Some(sha(&log).as_str())
                && support::command_log_valid(&log, &commands, &gate.commands, gate.result)
        });
        assert_eq!(now, *was, "{task} {run} {attempt} receipt validity");
        valid += usize::from(now);
    }
    println!(
        "gate receipts {}; valid before and after {valid}",
        before.len()
    );
}
