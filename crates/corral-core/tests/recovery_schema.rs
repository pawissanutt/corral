mod recovery_support;

use std::fs;
use std::process::{Command, Stdio};

use corral_core::{
    RecoveryLink, RecoveryReceipt, RecoveryReference, RecoveryState, Status, Store, TaskRecord,
};
use recovery_support::{append_outcome, recovery};
use serde_json::json;

const LEGACY: &str = "---\nid: T-001\ntitle: Legacy\nstatus: needs_me\ncreated: 2026-10-15T00:00:00Z\nupdated: 2026-10-15T00:00:00Z\n---\nOriginal specification.\n";

#[test]
fn full_record_lookup_refuses_snapshot_after_live_task_and_history_change() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Before", None, Status::NeedsMe, "").unwrap();
    let mut record = recovery(&store);
    store.save_recovery_record(&task.id, &record).unwrap();
    let snapshot = store.read_only_snapshot().unwrap();
    store
        .update(&task.id, |task| {
            task.title = "After".into();
            Ok(())
        })
        .unwrap();
    append_outcome(&mut record, RecoveryState::Applying);
    store.save_recovery_record(&task.id, &record).unwrap();
    // When
    let result = snapshot.task_record(&task.id);
    // Then
    assert!(result.is_err());
    assert_eq!(snapshot.get(&task.id).unwrap().title, "Before");
    let live = store.task_record(&task.id).unwrap();
    assert_eq!(live.task.title, "After");
    assert_eq!(live.recoveries, [record]);
}

#[test]
fn read_only_snapshot_refuses_recovery_write_without_mutating_store() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Snapshot", None, Status::NeedsMe, "").unwrap();
    let snapshot = store.read_only_snapshot().unwrap();
    let before = store.fixture_task_text(&task.id).unwrap().into_bytes();
    let record = recovery(&store);
    // When
    let result = snapshot.save_recovery_record(&task.id, &record);
    // Then
    assert!(result.is_err());
    assert_eq!(
        store.fixture_task_text(&task.id).unwrap().into_bytes(),
        before
    );
}

#[test]
fn every_legacy_document_loads_equal_to_its_legacy_parse() {
    // Given: a flat legacy task, one without its trailing newline, a schema-2 envelope and a
    // schema-3 envelope with recovery evidence.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let flat = Store::parse_task_record_markdown(LEGACY).unwrap();
    let schema2 = Store::render_task_markdown(&flat.task).replace("T-001", "T-002");
    let mut with_recovery = flat.clone();
    with_recovery.task.id = "T-003".into();
    let mut evidence = recovery(&store);
    evidence.preview.request.target.task = "T-003".into();
    append_outcome(&mut evidence, RecoveryState::Applying);
    with_recovery.recoveries = vec![evidence];
    let schema3 = Store::render_task_record_markdown(&with_recovery).unwrap();
    let unterminated = LEGACY.replace("T-001", "T-004");
    let unterminated = unterminated.trim_end_matches('\n');
    for (stem, text) in [
        ("T-001-legacy", LEGACY),
        ("T-002-schema2", schema2.as_str()),
        ("T-003-schema3", schema3.as_str()),
        ("T-004-unterminated", unterminated),
    ] {
        // When
        store.fixture_put_task_text(stem, text).unwrap();
        // Then
        let mut expected = Store::parse_task_record_markdown(text).unwrap();
        expected.task.path.clear();
        let id = expected.task.id.clone();
        assert_eq!(store.task_record(&id).unwrap(), expected, "{stem}");
        let reopened = Store::open_unobserved(tmp.path()).unwrap();
        assert_eq!(
            reopened.task_record(&id).unwrap(),
            expected,
            "{stem} reopened"
        );
    }
    assert_eq!(
        store.task_record("T-004").unwrap().task.body,
        "Original specification."
    );
    assert!(store.task_record("T-001").unwrap().recoveries.is_empty());
}

#[test]
fn recovery_writes_need_no_upgrade_and_refuse_rewritten_history_without_writing() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("History", None, Status::NeedsMe, "").unwrap();
    let mut record = recovery(&store);
    store.save_recovery_record(&task.id, &record).unwrap();
    append_outcome(&mut record, RecoveryState::Applying);
    record.links.push(RecoveryLink {
        recovery: RecoveryReference {
            operation: record.id.clone(),
            contract_digest: record.preview.contract_digest.clone(),
        },
        receipt: RecoveryReceipt::PlanReview {
            plan: "P1".into(),
            attempt: "PR2".into(),
        },
        acceptance: None,
    });
    store.save_recovery_record(&task.id, &record).unwrap();
    let stored = store.fixture_task_text(&task.id).unwrap();
    // When: intent changes, an outcome or a link disappears, or a retry repeats the record.
    let mut intent = record.clone();
    intent.authorization.reason = "A different reason".into();
    let mut dropped = record.clone();
    dropped.outcomes.pop();
    let mut unlinked = record.clone();
    unlinked.links.clear();
    // Then each rewrite is refused, the retry writes nothing, and the stored record holds.
    for (what, rewrite) in [("intent", intent), ("outcome", dropped), ("link", unlinked)] {
        assert!(
            store.save_recovery_record(&task.id, &rewrite).is_err(),
            "{what}"
        );
    }
    store.save_recovery_record(&task.id, &record).unwrap();
    assert_eq!(store.fixture_task_text(&task.id).unwrap(), stored);
    append_outcome(&mut record, RecoveryState::Applied);
    store.save_recovery_record(&task.id, &record).unwrap();
    let mut after_terminal = record.clone();
    append_outcome(&mut after_terminal, RecoveryState::Unknown);
    assert!(
        store
            .save_recovery_record(&task.id, &after_terminal)
            .is_err()
    );
    assert_eq!(store.task_record(&task.id).unwrap().recoveries, [record]);
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
    let artifact = store.dir().join("artifacts").join("T-001").join("R1");
    fs::create_dir_all(&artifact).unwrap();
    fs::write(artifact.join("gate.log"), "FAIL 1\n").unwrap();
    fs::write(artifact.join("plan.md"), "Original plan bytes\n").unwrap();
    let other = store
        .create("Unrelated", None, Status::Backlog, "")
        .unwrap();
    let other_bytes = store.fixture_task_text(&other.id).unwrap().into_bytes();
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
    let rendered = Store::render_task_record_markdown(&restored).unwrap();
    // Then
    assert_eq!(
        {
            let mut parsed = Store::parse_task_record_markdown(&rendered).unwrap();
            parsed.task.path = task.path.clone();
            parsed
        },
        restored
    );
    assert_eq!(restored.recoveries, [recovery]);
    assert_eq!(restored.task.runs, original.runs);
    assert_eq!(restored.task.body, original.body);
    assert_eq!(
        store.fixture_task_text(&other.id).unwrap().into_bytes(),
        other_bytes
    );
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
    let text = Store::render_task_record_markdown(&record).unwrap();
    // When / Then
    assert!(
        Store::parse_task_record_markdown(&text.replace("schema_version: 3", "schema_version: 2"))
            .is_err()
    );
    let legacy = LEGACY.replace("title: Legacy", "recoveries: []\ntitle: Legacy");
    assert!(Store::parse_task_markdown(&legacy).is_err());
    assert!(
        Store::parse_task_record_markdown(&text.replace("schema_version: 3", "schema_version: 4"))
            .is_err()
    );
    let mut invalid = record;
    invalid.recoveries[0].version = 2;
    assert!(Store::render_task_record_markdown(&invalid).is_err());
}

#[test]
fn future_store_fence_refuses_open_and_writes_without_task_effects() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Future", None, Status::NeedsMe, "").unwrap();
    let before = store.fixture_task_text(&task.id).unwrap().into_bytes();
    let future = corral_core::STORE_SCHEMA_VERSION + 1;
    let future_fence = format!("{future}\n");
    fs::write(
        store.dir().join("state").join("schema-version"),
        &future_fence,
    )
    .unwrap();
    // When / Then
    let refused = Store::open_unobserved(tmp.path()).unwrap_err();
    assert!(
        format!("{refused:#}").contains(&format!("schema {future}")),
        "{refused:#}"
    );
    assert!(store.save(&task).is_err());
    assert_eq!(
        store.fixture_task_text(&task.id).unwrap().into_bytes(),
        before
    );
    assert_eq!(
        fs::read_to_string(store.dir().join("state").join("schema-version")).unwrap(),
        future_fence
    );
}

#[test]
fn genuine_schema2_binary_refuses_a_database_store() {
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
    let before = store.fixture_task_text(&task.id).unwrap().into_bytes();
    // When: a pre-change writer opens the database store.
    let fresh = Command::new(binary)
        .arg(tmp.path())
        .env_clear()
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    // Then the fence refuses it and the task is untouched.
    assert!(!fresh.status.success());
    assert!(String::from_utf8_lossy(&fresh.stderr).contains("unsupported store schema 4"));
    assert_eq!(
        store.fixture_task_text(&task.id).unwrap().into_bytes(),
        before
    );
}

fn applied(record: &mut corral_core::RecoveryRecord) {
    append_outcome(record, RecoveryState::Applied);
    let effect = record.preview.proposed_effect.clone();
    record.outcomes.last_mut().unwrap().actual_effect = Some(effect);
}

#[test]
fn recovery_summary_counts_only_actually_applied_exceptions_and_keeps_unknown_distinct() {
    use corral_core::{HistorySummary, RecoverySummary};
    // A head written before the field existed is unknown, never "no recovery".
    let mut legacy = serde_json::to_value(HistorySummary::default()).unwrap();
    legacy.as_object_mut().unwrap().remove("recovery");
    let legacy: HistorySummary = serde_json::from_value(legacy).unwrap();
    assert_eq!(legacy.recovery, None);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("R", None, Status::NeedsMe, "Spec").unwrap();
    let summary = |store: &Store| store.head(&task.id).unwrap().summary.recovery;
    // A stored record with no recovery is known-empty.
    assert_eq!(summary(&store), Some(RecoverySummary::default()));
    // Intent and an Applied outcome without an actual effect are not applied exceptions.
    let mut early = recovery(&store);
    store.save_recovery_record(&task.id, &early).unwrap();
    let pending = summary(&store).unwrap();
    assert_eq!(
        (pending.records, pending.unsettled, pending.applied),
        (1, 1, 0)
    );
    assert_eq!(pending.check_overridden, 0);
    store.save_recovery_record(&task.id, &early).unwrap();
    assert_eq!(
        summary(&store).unwrap(),
        pending,
        "an identical retry changes nothing"
    );
    applied(&mut early);
    store.save_recovery_record(&task.id, &early).unwrap();
    let done = summary(&store).unwrap();
    assert_eq!(
        (done.applied, done.unsettled, done.check_overridden),
        (1, 0, 1)
    );
    assert_eq!(done.review_waived, 0);
    // A later failed and an uncertain operation keep the early applied override.
    for (id, state) in [
        ("REC2", RecoveryState::Failed),
        ("REC3", RecoveryState::Unknown),
    ] {
        let mut later = recovery(&store);
        later.id = id.into();
        append_outcome(&mut later, RecoveryState::Applying);
        append_outcome(&mut later, state);
        store.save_recovery_record(&task.id, &later).unwrap();
    }
    let mixed = summary(&store).unwrap();
    assert_eq!(
        (
            mixed.records,
            mixed.applied,
            mixed.failed,
            mixed.unknown,
            mixed.check_overridden
        ),
        (3, 1, 1, 1, 1)
    );
    // Ordinary head-native and full updates retain the known projection exactly.
    store
        .update(&task.id, |t| {
            t.title = "Ordinary edit".into();
            Ok(())
        })
        .unwrap();
    assert_eq!(summary(&store), Some(mixed.clone()));
    let record = store.task_record(&task.id).unwrap();
    assert_eq!(RecoverySummary::of(&record.recoveries), mixed);
    assert_eq!(HistorySummary::of(&record.task).unwrap().recovery, None);
}
