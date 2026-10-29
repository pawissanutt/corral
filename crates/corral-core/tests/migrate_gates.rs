//! Gate attempts (T-251 shapes, accepted cross-plan F3): legacy run, aggregate and checkpoint
//! gates import as the `gate_attempts` rows the v2 keyed readers use, under their original
//! textual attempt keys, with exact document bytes and no synthesized result.

#[path = "migrate_support/mod.rs"]
mod support;

use std::fs;

use corral_core::backend::{PageReq, RecordQuery, StoreBackend};
use corral_core::{CandidateIdentity, Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::{Legacy, done_task};

const RUN_ATTEMPT: &str = "4242-1792072974129040897";
const LOST_ATTEMPT: &str = "4343-1792072999000000000";
const AGGREGATE: &str = "ag-1792073000-7f3a";
const HEAD: &str = "1234567890123456789012345678901234567890";

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn identity(task: &str, run: &str, attempt: &str) -> Value {
    json!({
        "version": 2, "task": task, "run": run, "attempt": attempt, "head": HEAD,
        "tree": "2".repeat(40), "integration_base": "3".repeat(40), "dispatch_base": "4".repeat(40),
        "spec_digest": "5".repeat(64), "policy_digest": "6".repeat(64),
        "inputs_digest": "7".repeat(64), "environment": [["PATH", "/usr/bin"]]
    })
}

fn record(identity: Value, result: &str, log: Option<&[u8]>, failed: Option<&str>) -> Value {
    let mut gate = json!({
        "identity": identity, "result": result, "at": "2026-10-01T01:50:00Z", "sha": HEAD,
        "pane": "w9:p2"
    });
    if let Some(log) = log {
        gate["log_sha256"] = json!(sha(log));
        gate["commands"] = json!([{"command": "cargo test", "exit_code": 0, "log_start": 0,
            "log_end": log.len(), "started": "2026-10-01T01:45:00Z",
            "ended": "2026-10-01T01:50:00Z"}]);
    }
    if result == "running" {
        gate["process_lost"] = json!(true);
    }
    if let Some(failed) = failed {
        gate["failed_cmd"] = json!(failed);
    }
    gate
}

struct Files {
    run_log: Vec<u8>,
    run_result: Vec<u8>,
    run_context: Vec<u8>,
    lost_result: Vec<u8>,
    aggregate_log: Vec<u8>,
    checkpoint_log: Vec<u8>,
}

/// T-004 with a passed run gate (R1) and a process-lost one (R2), and T-005 with an aggregate
/// attempt and a checkpoint, each with the legacy files and events `main` writes.
fn sibling_shapes(legacy: &Legacy) -> Files {
    let files = Files {
        run_log: b"$ cargo test\nok\ncorral gate: pass\n".to_vec(),
        run_result: br#"{"result":"pass","exit_code":0}"#.to_vec(),
        run_context: br#"{"tier":"quick","tests":["package(corral-core)"]}"#.to_vec(),
        lost_result: br#"{"result":"pass","exit_code":0}"#.to_vec(),
        aggregate_log: b"$ scripts/gate.sh\ncorral gate: pass\n".to_vec(),
        checkpoint_log: b"$ cargo test\nFAILED\ncorral gate: fail\n".to_vec(),
    };
    let path = legacy.corral.join("tasks/T-004-gates.md");
    let mut task = corral_core::parse_task_record(&done_task("T-004", "unused"), &path)
        .unwrap()
        .task;
    task.artifacts.clear();
    let run_identity = identity("T-004", "R1", RUN_ATTEMPT);
    task.runs[0].gate = Some(
        serde_json::from_value(record(
            run_identity.clone(),
            "pass",
            Some(&files.run_log),
            None,
        ))
        .unwrap(),
    );
    let mut lost = task.runs[0].clone();
    lost.id = "R2".to_owned();
    lost.gate = Some(
        serde_json::from_value(record(
            identity("T-004", "R2", LOST_ATTEMPT),
            "running",
            None,
            None,
        ))
        .unwrap(),
    );
    task.runs.push(lost);
    fs::write(&path, corral_core::render_task(&task)).unwrap();

    let path = legacy.corral.join("tasks/T-005-plan.md");
    let mut plan = corral_core::parse_task_record(&done_task("T-005", "unused"), &path)
        .unwrap()
        .task;
    plan.artifacts.clear();
    plan.runs.clear();
    plan.aggregate_attempts.push(
        serde_json::from_value(json!({
            "id": AGGREGATE, "identity": identity("T-005", "aggregate", AGGREGATE),
            "target": {"kind": "main"}, "code": false, "members": [], "started": "2026-10-01T02:00:00Z",
            "gate": record(identity("T-005", "aggregate", AGGREGATE), "pass",
                Some(&files.aggregate_log), None)
        }))
        .unwrap(),
    );
    fs::write(&path, corral_core::render_task(&plan)).unwrap();
    let journal = json!({"version": 1, "attempts": [{
        "id": "C1", "advisory": true, "automatic": false, "target": "plan/x", "head": HEAD,
        "landings": 1, "started": "2026-10-01T03:00:00Z",
        "identity": identity("T-005", "checkpoint", "C1"),
        "gate": record(identity("T-005", "checkpoint", "C1"), "fail",
            Some(&files.checkpoint_log), Some("cargo test"))
    }]});
    legacy.put(
        "state/plan-checkpoints/T-005.json",
        &serde_json::to_vec(&journal).unwrap(),
    );

    let stem = format!("state/gates/T-004-R1-{RUN_ATTEMPT}");
    legacy.put(&format!("{stem}.log"), &files.run_log);
    legacy.put(&format!("{stem}.json"), &files.run_result);
    legacy.put(&format!("{stem}.context.json"), &files.run_context);
    legacy.put(
        &format!("state/gates/T-004-R2-{LOST_ATTEMPT}.json"),
        &files.lost_result,
    );
    legacy.put(
        &format!("state/plan-gates/T-005-{AGGREGATE}.log"),
        &files.aggregate_log,
    );
    legacy.put("state/plan-gates/T-005-C1.log", &files.checkpoint_log);
    let events = [
        json!({"ts": "2026-10-01T01:45:00Z", "task": "T-004", "kind": "gate_started",
            "detail": {"run": "R1", "sha": HEAD, "pane": "w9:p2", "commands": ["cargo test"],
                "identity": run_identity}}),
        json!({"ts": "2026-10-01T01:50:00Z", "task": "T-004", "kind": "gate",
            "detail": {"run": "R1", "sha": HEAD, "result": "pass", "failed_cmd": null,
                "identity": identity("T-004", "R1", RUN_ATTEMPT)}}),
        json!({"ts": "2026-10-01T01:55:00Z", "task": "T-004", "kind": "gate_started",
            "detail": {"run": "R2", "sha": HEAD, "pane": "w9:p3", "commands": ["cargo test"],
                "identity": identity("T-004", "R2", LOST_ATTEMPT)}}),
    ];
    let mut text = fs::read_to_string(legacy.corral.join("events.jsonl")).unwrap();
    for event in events {
        text.push_str(&format!("{event}\n"));
    }
    fs::write(legacy.corral.join("events.jsonl"), text).unwrap();
    files
}

fn bytes(store: &Store, blob: Option<&corral_core::blob_ref::BlobRef>) -> Option<Vec<u8>> {
    blob.map(|blob| store.blob_bytes(blob).unwrap().unwrap())
}

#[test]
fn legacy_gates_import_as_keyed_attempt_rows() {
    // Given run, process-lost, aggregate and checkpoint gates in the legacy shapes.
    let legacy = Legacy::new();
    let files = sibling_shapes(&legacy);
    // When the store is migrated.
    let applied = legacy.migrate().apply().unwrap();
    assert_eq!(applied.import.unwrap().gate_attempts, 4);
    let store = Store::open(&legacy.root).unwrap();
    // Then the run attempt reads back by its textual key with the exact documents, and its
    // log still matches the receipt's log_sha256 (the ordinary gate validation's binding).
    let row = store
        .gate_attempt("T-004", "R1", RUN_ATTEMPT)
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.scope.as_str(), row.result.as_deref()),
        ("run", Some("pass"))
    );
    assert_eq!(row.launched, Some(json!("w9:p2")));
    assert!(row.ended.is_some());
    let log = bytes(&store, row.log.as_ref()).unwrap();
    assert_eq!(log, files.run_log);
    let task = store.get("T-004").unwrap();
    let receipt = task.runs[0].gate.as_ref().unwrap();
    assert_eq!(receipt.log_sha256.as_deref(), Some(sha(&log).as_str()));
    assert_eq!(
        bytes(&store, row.result_doc.as_ref()),
        Some(files.run_result)
    );
    assert_eq!(bytes(&store, row.context.as_ref()), Some(files.run_context));
    let identity: &CandidateIdentity = receipt.identity.as_ref().unwrap();
    assert_eq!(
        bytes(&store, Some(&row.identity)),
        Some(serde_json::to_vec(identity).unwrap())
    );
    assert_eq!(
        bytes(&store, Some(&row.commands)),
        Some(br#"["cargo test"]"#.to_vec())
    );
    // And the process-lost attempt has no synthesized result: unended, its result file kept.
    let lost = store
        .gate_attempt("T-004", "R2", LOST_ATTEMPT)
        .unwrap()
        .unwrap();
    assert_eq!((lost.result.as_deref(), lost.ended), (None, None));
    assert_eq!(
        bytes(&store, lost.result_doc.as_ref()),
        Some(files.lost_result)
    );
    let receipt = task.runs[1].gate.as_ref().unwrap();
    assert!(receipt.process_lost && receipt.result == corral_core::GateResult::Running);
    // And the aggregate (`ag-...`) and checkpoint attempts are keyed by scope on their Plan.
    let aggregate = store
        .gate_attempt("T-005", "aggregate", AGGREGATE)
        .unwrap()
        .unwrap();
    assert_eq!(
        (aggregate.scope.as_str(), aggregate.result.as_deref()),
        ("aggregate", Some("pass"))
    );
    assert_eq!(
        bytes(&store, aggregate.log.as_ref()),
        Some(files.aggregate_log)
    );
    let checkpoint = store
        .gate_attempt("T-005", "checkpoint", "C1")
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            checkpoint.scope.as_str(),
            checkpoint.result.as_deref(),
            checkpoint.failed_cmd.as_deref()
        ),
        ("checkpoint", Some("fail"), Some("cargo test"))
    );
    assert_eq!(
        bytes(&store, checkpoint.log.as_ref()),
        Some(files.checkpoint_log)
    );
    // And the checkpoint journal is the Plan's `plan-checkpoints` record.
    drop(store);
    let backend = corral_core::sqlite::SqliteBackend::open(
        &legacy.corral,
        corral_core::sqlite::SqliteOptions::default(),
    )
    .unwrap();
    let query = RecordQuery {
        area: Some("plan-checkpoints".to_owned()),
        key: Some("T-005".to_owned()),
    };
    let page = backend
        .view()
        .unwrap()
        .records(&query, PageReq::first(2))
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].doc["value"]["attempts"][0]["id"], "C1");
}
