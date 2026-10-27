//! Gate evidence on storage v2 (plan T-253, evidence-gates): control files in a registered
//! scratch directory, exact-byte blobs in one `gate_attempts` row, slim events.
use std::path::{Path, PathBuf};
use std::process::Command;

use corral_core::backend::{ScratchKind, ScratchRow, ScratchState};
use corral_core::{GateRecord, GateResult};
use corral_ops::Ctx;
use serde_json::{Value, json};
use sha2::Digest;

use super::{git, running_task};

fn scratch(ctx: &Ctx, id: &str, attempt: &str) -> ScratchRow {
    ctx.store
        .scratch_row(&format!("gate-{id}-R1-{attempt}"))
        .unwrap()
        .expect("registered gate scratch")
}

fn attempt(record: &GateRecord) -> String {
    record.identity.as_ref().unwrap().attempt.clone()
}

fn gate(ctx: &Ctx, id: &str) -> (GateRecord, PathBuf) {
    corral_ops::gate_start(ctx, id).unwrap();
    let record = corral_ops::gate_wait(ctx, id).unwrap();
    let dir = corral_ops::gate_paths(ctx, id, "R1")
        .log
        .parent()
        .unwrap()
        .to_path_buf();
    (record, dir)
}

fn commit(worktree: &Path, file: &str) {
    std::fs::write(worktree.join(file), "x\n").unwrap();
    git(worktree, &["add", "-A"]);
    git(worktree, &["commit", "-q", "-m", file]);
}

fn events(ctx: &Ctx, id: &str, kind: &str) -> Vec<Value> {
    ctx.store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.task.as_deref() == Some(id) && e.kind == kind)
        .map(|e| e.detail)
        .collect()
}

#[test]
fn gate_control_files_sit_in_registered_scratch_and_collect_into_one_row() {
    let (_tmp, ctx, id, _worktree) = running_task(&["echo hello-gate"]);
    // When a gate starts (the fake pane runs it to completion synchronously).
    let started = corral_ops::gate_start(&ctx, &id).unwrap();
    let identity = started.identity.clone().unwrap();
    // Then its control files and live log sit in the attempt's registered scratch directory.
    let paths = corral_ops::gate_paths(&ctx, &id, "R1");
    let layout = ctx.store.storage_layout().unwrap();
    for path in [&paths.script, &paths.log, &paths.result] {
        assert!(path.is_file(), "{}", path.display());
        assert!(path.starts_with(layout.scratch_root().join("gates")));
        assert!(!path.starts_with(ctx.store.dir()), "{}", path.display());
    }
    assert!(!ctx.store.dir().join("state/gates").exists());
    let registered = scratch(&ctx, &id, &identity.attempt);
    assert_eq!(
        (registered.kind, registered.state),
        (ScratchKind::Gate, ScratchState::Open)
    );
    // And the started row holds the identity and commands as exact bytes, with its pane.
    let row = ctx
        .store
        .gate_attempt(&id, "R1", &identity.attempt)
        .unwrap()
        .unwrap();
    let blob = |blob| ctx.store.blob_bytes(blob).unwrap().unwrap();
    assert_eq!(row.scope, "run");
    assert_eq!(blob(&row.identity), serde_json::to_vec(&identity).unwrap());
    assert_eq!(blob(&row.commands), br#"["echo hello-gate"]"#);
    assert_eq!(row.launched, Some(json!(started.pane)));
    assert!(row.ended.is_none() && row.log.is_none() && row.result_doc.is_none());
    // When it is collected.
    let record = corral_ops::gate_collect(&ctx, &id).unwrap().unwrap();
    assert_eq!(record.result, GateResult::Pass);
    // Then the one row is ended with the log and result documents' exact bytes.
    let row = ctx
        .store
        .gate_attempt(&id, "R1", &identity.attempt)
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.result.as_deref(), row.failed_cmd.as_deref()),
        (Some("pass"), None)
    );
    assert!(row.ended.is_some());
    let log = blob(row.log.as_ref().unwrap());
    assert_eq!(log, std::fs::read(&paths.log).unwrap());
    assert_eq!(
        blob(row.result_doc.as_ref().unwrap()),
        std::fs::read(&paths.result).unwrap()
    );
    assert_eq!(
        record.log_sha256,
        Some(format!("{:x}", sha2::Sha256::digest(&log)))
    );
    // And the scratch directory is closed, not yet reclaimed.
    assert_eq!(
        scratch(&ctx, &id, &identity.attempt).state,
        ScratchState::Closed
    );
    assert!(paths.log.is_file());
}

#[test]
fn a_closed_gate_directory_is_reclaimed_only_after_its_runner_exits() {
    let (_tmp, ctx, id, worktree) = running_task(&["true"]);
    let (first, first_dir) = gate(&ctx, &id);
    // Given a process standing in for a runner that still works in the closed directory.
    let mut runner = Command::new("sleep")
        .arg("600")
        .current_dir(&first_dir)
        .spawn()
        .unwrap();
    // When the next gate starts.
    commit(&worktree, "a.txt");
    let (second, second_dir) = gate(&ctx, &id);
    runner.kill().unwrap();
    runner.wait().unwrap();
    // Then the busy directory is deferred with its reason and kept.
    let deferred = scratch(&ctx, &id, &attempt(&first));
    assert_eq!(deferred.state, ScratchState::Deferred);
    assert!(
        deferred.reason.as_deref().unwrap().contains("process"),
        "{deferred:?}"
    );
    assert!(first_dir.is_dir());
    // When a later gate starts once nothing uses the previous directory.
    commit(&worktree, "b.txt");
    gate(&ctx, &id);
    // Then that directory is reclaimed, and its evidence outlives it in the row.
    assert_eq!(
        scratch(&ctx, &id, &attempt(&second)).state,
        ScratchState::Reclaimed
    );
    assert!(!second_dir.exists());
    let row = ctx
        .store
        .gate_attempt(&id, "R1", &attempt(&second))
        .unwrap()
        .unwrap();
    let log = ctx.store.blob_bytes(row.log.as_ref().unwrap()).unwrap();
    assert!(log.unwrap().ends_with(b"corral gate: pass\n"));
}

#[test]
fn the_recorded_log_blob_not_the_scratch_copy_proves_a_pass() {
    let (_tmp, ctx, id, _worktree) = running_task(&["echo proof"]);
    let (record, _) = gate(&ctx, &id);
    assert_eq!(record.result, GateResult::Pass);
    // Given the scratch copy of the log is tampered with after collection.
    let paths = corral_ops::gate_paths(&ctx, &id, "R1");
    std::fs::write(&paths.log, "\n$ echo proof\nforged\n\ncorral gate: pass\n").unwrap();
    // When the receipt is checked again, its byte ranges and digest are over the blob.
    let again = corral_ops::gate_collect(&ctx, &id).unwrap().unwrap();
    // Then the pass stands, exactly as with the untouched file log.
    assert_eq!(again, record);
    assert!(events(&ctx, &id, "gate_invalidated").is_empty());
}

#[test]
fn slim_gate_events_name_the_attempt_and_analytics_read_the_row() {
    let (_tmp, ctx, id, _worktree) = running_task(&["echo step"]);
    let (record, _) = gate(&ctx, &id);
    let attempt = attempt(&record);
    // Then the start event names the attempt, pane and behind-at-start, not commands or identity.
    let started = events(&ctx, &id, "gate_started").pop().unwrap();
    assert_eq!(started["attempt"], json!(attempt));
    assert_eq!(started["behind_at_start"], json!(false));
    assert_eq!(started["pane"], json!(record.pane));
    assert_eq!(started["sha"], json!(record.sha));
    for field in ["commands", "identity"] {
        assert!(started.get(field).is_none(), "{field}: {started}");
    }
    // And the collection event carries the result and failed command.
    let collected = events(&ctx, &id, "gate").pop().unwrap();
    assert_eq!(collected["attempt"], json!(attempt));
    assert_eq!(
        (collected["result"].clone(), collected["failed_cmd"].clone()),
        (json!("pass"), Value::Null)
    );
    for field in ["commands", "identity"] {
        assert!(collected.get(field).is_none(), "{field}: {collected}");
    }
    // And `stats --gates` pairs them by attempt and reads the receipts from the row.
    let report = serde_json::to_value(
        corral_ops::analytics::gate_metrics::gate_stats(&ctx.store, None, None).unwrap(),
    )
    .unwrap();
    let totals = &report["totals"];
    assert_eq!(report["gate_events_unmatched"], 0);
    assert_eq!(
        (
            totals["gates_started"].clone(),
            totals["gates_finished"].clone()
        ),
        (json!(1), json!(1))
    );
    assert_eq!(totals["behind"]["contained_base"]["gates"], 1);
    assert_eq!(totals["full_step_seconds"]["echo step"]["samples"], 1);
}
