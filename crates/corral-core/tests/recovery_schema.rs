mod recovery_support;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use corral_core::{
    RecoveryState, Status, Store, TaskRecord, parse_task, parse_task_record, render_task_record,
};
use recovery_support::{append_outcome, recovery};
use serde_json::json;

const LEGACY: &str = "---\nid: T-001\ntitle: Legacy\nstatus: needs_me\ncreated: 2026-10-15T00:00:00Z\nupdated: 2026-10-15T00:00:00Z\n---\nOriginal specification.\n";

#[test]
fn read_only_snapshot_refuses_schema_upgrade_without_mutating_store() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Snapshot", None, Status::NeedsMe, "").unwrap();
    let snapshot = store.read_only_snapshot().unwrap();
    let before = fs::read(&task.path).unwrap();
    // When
    let result = snapshot.upgrade_schema();
    // Then
    assert!(result.is_err());
    assert_eq!(fs::read(&task.path).unwrap(), before);
    assert_eq!(
        fs::read(store.state_dir().join("schema-version")).unwrap(),
        b"2\n"
    );
    assert!(!store.state_dir().join("schema-3-backup").exists());
}

#[test]
fn read_only_snapshot_refuses_recovery_write_without_mutating_store() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Snapshot", None, Status::NeedsMe, "").unwrap();
    store.upgrade_schema().unwrap();
    let snapshot = store.read_only_snapshot().unwrap();
    let before = fs::read(&task.path).unwrap();
    let record = recovery(&store);
    // When
    let result = snapshot.save_recovery_record(&task.id, &record);
    // Then
    assert!(result.is_err());
    assert_eq!(fs::read(&task.path).unwrap(), before);
}

#[test]
fn legacy_specification_bytes_without_trailing_newline_survive_upgrade() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Legacy", None, Status::NeedsMe, "").unwrap();
    let text = LEGACY.trim_end_matches('\n');
    fs::write(&task.path, text).unwrap();
    let original = store.get(&task.id).unwrap();
    // When
    store.upgrade_schema().unwrap();
    // Then
    let record = store.task_record(&task.id).unwrap();
    assert_eq!(record.task.body.as_bytes(), original.body.as_bytes());
    let rendered = render_task_record(&record).unwrap();
    assert_eq!(parse_task_record(&rendered, &task.path).unwrap(), record);
}

#[test]
fn legacy_and_schema2_read_without_upgrading_or_inventing_recovery() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Legacy", None, Status::NeedsMe, "").unwrap();
    fs::write(&task.path, LEGACY).unwrap();
    let marker = store.state_dir().join("schema-version");
    let before = fs::read(&marker).unwrap();
    let events = fs::read(store.dir().join("events.jsonl")).unwrap();
    // When
    let reopened = Store::open_unobserved(tmp.path()).unwrap();
    let record = reopened.task_record(&task.id).unwrap();
    // Then
    assert!(record.recoveries.is_empty());
    assert_eq!(record.task.body, "Original specification.\n");
    assert_eq!(fs::read(&task.path).unwrap(), LEGACY.as_bytes());
    assert_eq!(fs::read(&marker).unwrap(), before);
    assert_eq!(fs::read(store.dir().join("events.jsonl")).unwrap(), events);
    let schema2 = format!(
        "---\nschema_version: 2\ntask:\n{}---\nbody\n",
        serde_yaml_ng::to_string(&record.task)
            .unwrap()
            .lines()
            .map(|line| format!("  {line}\n"))
            .collect::<String>()
    );
    assert_eq!(
        parse_task_record(&schema2, &task.path).unwrap().task.body,
        "body\n"
    );
}

#[test]
fn ordinary_writes_keep_unupgraded_store_at_schema2() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Ordinary", None, Status::Backlog, "").unwrap();
    // When
    store
        .update(&task.id, |t| {
            t.title = "Changed".into();
            Ok(())
        })
        .unwrap();
    // Then
    assert_eq!(
        fs::read_to_string(store.state_dir().join("schema-version")).unwrap(),
        "2\n"
    );
    let text = fs::read_to_string(&task.path).unwrap();
    let yaml: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(text.split("---\n").nth(1).unwrap()).unwrap();
    assert_eq!(yaml["schema_version"].as_u64(), Some(2));
}

#[test]
fn recovery_mutation_refuses_before_explicit_upgrade() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Blocked", None, Status::NeedsMe, "").unwrap();
    let before = fs::read(&task.path).unwrap();
    let record = recovery(&store);
    // When
    let result = store.save_recovery_record(&task.id, &record);
    // Then
    assert!(result.is_err());
    assert_eq!(fs::read(&task.path).unwrap(), before);
    assert_eq!(
        fs::read_to_string(store.state_dir().join("schema-version")).unwrap(),
        "2\n"
    );
}

#[test]
fn explicit_upgrade_backs_up_original_bytes_and_preserves_original_evidence() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("Recovery", None, Status::NeedsMe, "Original spec")
        .unwrap();
    let other = store
        .create("Unrelated", None, Status::Backlog, "Unrelated spec")
        .unwrap();
    let originals = [store.get(&task.id).unwrap(), store.get(&other.id).unwrap()];
    let bytes = originals
        .iter()
        .map(|t| fs::read(&t.path).unwrap())
        .collect::<Vec<_>>();
    let config = fs::read(store.config_path()).unwrap();
    let events = fs::read(store.dir().join("events.jsonl")).unwrap();
    // When
    let backup = store.upgrade_schema().unwrap();
    // Then
    for (original, bytes) in originals.iter().zip(&bytes) {
        assert_eq!(
            fs::read(
                backup
                    .join("tasks")
                    .join(original.path.file_name().unwrap())
            )
            .unwrap(),
            *bytes
        );
        assert_eq!(store.get(&original.id).unwrap(), *original);
    }
    assert_eq!(fs::read(backup.join("schema-version")).unwrap(), b"2\n");
    assert_eq!(
        fs::read_to_string(store.state_dir().join("schema-version")).unwrap(),
        "3\n"
    );
    assert_eq!(fs::read(store.config_path()).unwrap(), config);
    assert_eq!(fs::read(store.dir().join("events.jsonl")).unwrap(), events);
    assert_eq!(store.upgrade_schema().unwrap(), backup);
    assert_eq!(
        fs::read(backup.join("tasks").join(task.path.file_name().unwrap())).unwrap(),
        bytes[0]
    );
}

#[test]
fn malformed_tasks_abort_upgrade_before_backup_or_marker_mutation() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Valid", None, Status::Backlog, "").unwrap();
    fs::write(
        store.dir().join("tasks/T-002-invalid.md"),
        "---\nschema_version: 999\ntask: {}\n---\n",
    )
    .unwrap();
    let before = fs::read(&task.path).unwrap();
    // When
    let result = store.upgrade_schema();
    // Then
    assert!(result.is_err());
    assert_eq!(fs::read(&task.path).unwrap(), before);
    assert_eq!(
        fs::read_to_string(store.state_dir().join("schema-version")).unwrap(),
        "2\n"
    );
    assert!(!store.state_dir().join("schema-3-backup").exists());
}

#[test]
fn interrupted_envelope_conversion_resumes_without_replacing_backup() {
    // Given: crash after schema marker publication but before an envelope rewrite.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Resume", None, Status::NeedsMe, "").unwrap();
    let before = fs::read(&task.path).unwrap();
    let backup = store.upgrade_schema().unwrap();
    fs::write(&task.path, &before).unwrap();
    // When
    store.upgrade_schema().unwrap();
    // Then
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(
        fs::read_to_string(&task.path)
            .unwrap()
            .split("---\n")
            .nth(1)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(yaml["schema_version"].as_u64(), Some(3));
    assert_eq!(
        fs::read(backup.join("tasks").join(task.path.file_name().unwrap())).unwrap(),
        before
    );
}

#[test]
fn sealed_backup_can_resume_before_marker_publication_but_cannot_hide_changed_tasks() {
    // Given: a complete backup followed by loss before publishing the new marker.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("Before marker", None, Status::NeedsMe, "")
        .unwrap();
    let before = fs::read(&task.path).unwrap();
    let backup = store.upgrade_schema().unwrap();
    fs::write(&task.path, &before).unwrap();
    fs::write(store.state_dir().join("schema-version"), "2\n").unwrap();
    // When
    store.upgrade_schema().unwrap();
    // Then
    assert_eq!(
        fs::read(backup.join("tasks").join(task.path.file_name().unwrap())).unwrap(),
        before
    );
    assert_eq!(
        fs::read_to_string(store.state_dir().join("schema-version")).unwrap(),
        "3\n"
    );
}

#[test]
fn conflicting_backup_refuses_without_overwriting_originals() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Before", None, Status::NeedsMe, "").unwrap();
    let backup = store.state_dir().join("schema-3-backup/tasks");
    fs::create_dir_all(&backup).unwrap();
    let saved = backup.join(task.path.file_name().unwrap());
    fs::write(&saved, "retained conflicting bytes").unwrap();
    let before = fs::read(&task.path).unwrap();
    // When
    let result = store.upgrade_schema();
    // Then
    assert!(result.is_err());
    assert_eq!(fs::read(&saved).unwrap(), b"retained conflicting bytes");
    assert_eq!(fs::read(&task.path).unwrap(), before);
    assert_eq!(
        fs::read_to_string(store.state_dir().join("schema-version")).unwrap(),
        "2\n"
    );
}

#[test]
fn recovery_round_trip_and_ordinary_updates_preserve_the_complete_chain() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Recovery", None, Status::NeedsMe, "Original specification")
        .unwrap();
    let mut task_json = serde_json::to_value(&task).unwrap();
    task_json["runs"] = json!([{
        "id": "R1", "agent": "worker", "worktree": "/fixture/worktree",
        "branch": "corral/T-001", "base": "main", "base_sha": "base",
        "started": "2026-10-18T10:00:00Z",
        "gate": {"result": "fail", "failed_cmd": "fixture-gate", "at": "2026-10-18T10:00:00Z",
            "sha": "head", "commands": [{"command": "fixture-gate", "exit_code": 1, "log_start": 0, "log_end": 8}]},
        "review": {"reviewer": "real-reviewer", "verdict": "changes", "notes": "Original review notes",
            "sha": "head", "at": "2026-10-18T10:00:00Z"}
    }]);
    task.runs = serde_json::from_value(task_json["runs"].clone()).unwrap();
    store.save(&task).unwrap();
    let original = store.get(&task.id).unwrap();
    let artifact = store.artifacts_dir("T-001", "R1");
    fs::create_dir_all(&artifact).unwrap();
    fs::write(artifact.join("gate.log"), "FAIL 1\n").unwrap();
    fs::write(artifact.join("plan.md"), "Original plan bytes\n").unwrap();
    let other = store
        .create("Unrelated", None, Status::Backlog, "")
        .unwrap();
    store.upgrade_schema().unwrap();
    let other_bytes = fs::read(&other.path).unwrap();
    let config_bytes = fs::read(store.config_path()).unwrap();
    let mut recovery = recovery(&store);
    append_outcome(&mut recovery, RecoveryState::Applying);
    append_outcome(&mut recovery, RecoveryState::Unknown);
    store.save_recovery_record(&task.id, &recovery).unwrap();
    // When
    store
        .update(&task.id, |t| {
            t.title = "Ordinary edit".into();
            Ok(())
        })
        .unwrap();
    let reopened = Store::open_unobserved(tmp.path()).unwrap();
    let restored = reopened.task_record(&task.id).unwrap();
    let rendered = render_task_record(&restored).unwrap();
    // Then
    assert_eq!(parse_task_record(&rendered, &task.path).unwrap(), restored);
    assert_eq!(restored.recoveries, [recovery]);
    assert_eq!(restored.task.runs, original.runs);
    assert_eq!(restored.task.body, original.body);
    assert_eq!(fs::read(&other.path).unwrap(), other_bytes);
    assert_eq!(fs::read(store.config_path()).unwrap(), config_bytes);
    assert_eq!(fs::read(artifact.join("gate.log")).unwrap(), b"FAIL 1\n");
    assert_eq!(
        fs::read(artifact.join("plan.md")).unwrap(),
        b"Original plan bytes\n"
    );
}

#[test]
fn recovery_cannot_be_hidden_in_legacy_or_schema2_envelopes() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Envelope", None, Status::NeedsMe, "").unwrap();
    let record = TaskRecord {
        task,
        recoveries: vec![recovery(&store)],
    };
    let text = render_task_record(&record).unwrap();
    // When / Then
    assert!(
        parse_task_record(
            &text.replace("schema_version: 3", "schema_version: 2"),
            Path::new("T-001.md")
        )
        .is_err()
    );
    let legacy = LEGACY.replace("title: Legacy", "recoveries: []\ntitle: Legacy");
    assert!(parse_task(&legacy, Path::new("T-001.md")).is_err());
    assert!(
        parse_task_record(
            &text.replace("schema_version: 3", "schema_version: 4"),
            Path::new("T-001.md")
        )
        .is_err()
    );
    let mut invalid = record;
    invalid.recoveries[0].version = 2;
    assert!(render_task_record(&invalid).is_err());
}

#[test]
fn future_store_schema_refuses_open_and_upgrade_without_task_effects() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Future", None, Status::NeedsMe, "").unwrap();
    let before = fs::read(&task.path).unwrap();
    fs::write(store.state_dir().join("schema-version"), "4\n").unwrap();
    // When / Then
    assert!(Store::open_unobserved(tmp.path()).is_err());
    assert!(store.upgrade_schema().is_err());
    assert!(store.save(&task).is_err());
    assert_eq!(fs::read(&task.path).unwrap(), before);
    assert_eq!(
        fs::read_to_string(store.state_dir().join("schema-version")).unwrap(),
        "4\n"
    );
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
        }
        self.0.wait().unwrap();
    }
}

#[test]
fn genuine_schema2_binary_refuses_reopen_and_already_open_write_after_upgrade() {
    // Given: executable linked against the pre-change core, never the candidate core.
    let Some(binary) = std::env::var_os("CORRAL_SCHEMA2_WRITER") else {
        eprintln!(
            "not verified: set CORRAL_SCHEMA2_WRITER to the preserved schema-2 fixture executable"
        );
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("Old writer", None, Status::NeedsMe, "")
        .unwrap();
    let mut command = Command::new(&binary);
    command
        .arg(tmp.path())
        .env_clear()
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ChildGuard(command.spawn().unwrap());
    let stdout = child.0.stdout.take().unwrap();
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(15)).unwrap(),
        "schema2-ready"
    );
    // When: the already-open old writer observes the new barrier before its save.
    store.upgrade_schema().unwrap();
    let before = fs::read(&task.path).unwrap();
    child
        .0
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"save\n")
        .unwrap();
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(15)).unwrap(),
        "schema2-refused"
    );
    // Then
    assert_eq!(fs::read(&task.path).unwrap(), before);
    drop(child);
    reader.join().unwrap();
    let fresh = Command::new(binary)
        .arg(tmp.path())
        .env_clear()
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!fresh.status.success());
    assert!(String::from_utf8_lossy(&fresh.stderr).contains("unsupported store schema 3"));
    assert_eq!(fs::read(&task.path).unwrap(), before);
}
