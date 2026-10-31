//! `corral migrate` forward path: dry run, the drain check, A1-A7 with the import checks, the
//! forward barriers B1-B7 and the injected states I5-I6 (plan T-253 section 12).

#[path = "migrate_support/mod.rs"]
mod support;

use std::fs;

use corral_core::backend::HistoryRow;
use corral_core::migrate::JournalState;
use support::{FENCE, Legacy, kill_at, killed};

#[test]
fn dry_run_classifies_every_file_and_reports_lost_gates_as_history() {
    // Given a drained legacy board with a process-lost gate on a Done task.
    let legacy = Legacy::new();
    let before = legacy.legacy_bytes();
    // When the dry run reads it.
    let report = legacy.migrate().dry_run().unwrap();
    // Then nothing blocks, the lost gate is history and every file is classified.
    assert!(report.ok, "{report:?}");
    assert!(report.blockers.is_empty());
    assert_eq!(report.history.len(), 1);
    assert_eq!(report.history[0].family, "process_lost_gate");
    assert!(report.unclassified.is_empty());
    assert_eq!(report.tasks_by_schema.get("2"), Some(&2));
    assert_eq!(report.tasks_by_schema.get("flat"), Some(&1));
    assert_eq!(report.event_lines, 5);
    assert_eq!(report.build_output.len(), 1);
    // And it created nothing: no journal, the same fence and the same legacy bytes.
    assert!(legacy.journal().is_none());
    assert_eq!(legacy.fence().as_deref(), Some(FENCE));
    assert_eq!(legacy.legacy_bytes(), before);
    assert!(!legacy.exists("db") && !legacy.exists("db.migrating") && !legacy.exists("blobs"));
}

#[test]
fn dry_run_lists_and_refuses_unclassified_files() {
    // Given a legacy board with a state file no area declares.
    let legacy = Legacy::new();
    fs::write(legacy.corral.join("state/unknown-thing.bin"), b"?").unwrap();
    // When the dry run and an apply read it.
    let report = legacy.migrate().dry_run().unwrap();
    // Then the file is listed, the dry run is not ok and the apply is refused unchanged.
    assert_eq!(report.unclassified, ["state/unknown-thing.bin"]);
    assert!(!report.ok);
    let error = legacy.migrate().apply().unwrap_err();
    assert!(
        format!("{error:#}").contains("state/unknown-thing.bin"),
        "{error:#}"
    );
    assert!(legacy.journal().is_none());
    assert_eq!(legacy.fence().as_deref(), Some(FENCE));
}

#[test]
fn new_main_assistant_and_pm_desk_legacy_roots_refuse_without_discarding_bytes() {
    // These incoming-main roots have no declared importer. Never silently treat their
    // sessions, containers or desk items as rebuildable caches or generic evidence.
    for path in [
        "assistants/A-1.md",
        "state/agent-assistant/A-1.json",
        "state/assistant-counter",
        "pm-inbox/PMI-1.md",
        "state/pm-inbox-counter",
    ] {
        let legacy = Legacy::new();
        let target = legacy.corral.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, format!("retained incoming-main data: {path}\n")).unwrap();
        let before = legacy.legacy_bytes();
        let report = legacy.migrate().dry_run().unwrap();
        assert!(!report.ok, "{path}");
        assert_eq!(report.unclassified, [path]);
        let error = legacy.migrate().apply().unwrap_err();
        assert!(format!("{error:#}").contains(path), "{path}: {error:#}");
        assert_eq!(legacy.legacy_bytes(), before, "{path}");
        assert!(legacy.journal().is_none(), "{path}");
        assert_eq!(legacy.fence().as_deref(), Some(FENCE), "{path}");
        assert!(
            !legacy.exists("db") && !legacy.exists("db.migrating") && !legacy.exists("blobs"),
            "{path}"
        );
    }
}

fn inject(legacy: &Legacy, family: &str) {
    let tasks = legacy.corral.join("tasks");
    let open_run = "  runs:\n  - id: R1\n    agent: claude\n    worktree: /tmp/w\n    branch: corral/T-009\n    base: main\n    base_sha: '00'\n    started: 2026-10-04T00:00:00Z\n";
    let ended_run = "  runs:\n  - id: R1\n    agent: claude\n    worktree: /tmp/w\n    branch: corral/T-009\n    base: main\n    base_sha: '00'\n    started: 2026-10-04T00:00:00Z\n    ended: 2026-10-04T01:00:00Z\n";
    let head = "---\nschema_version: 2\ntask:\n  id: T-009\n  title: Injected\n  status: done\n  priority: P2\n  created: 2026-10-04T00:00:00Z\n  updated: 2026-10-04T00:00:00Z\n";
    let body = match family {
        "open_coding_run" => open_run.to_owned(),
        "open_review_run" => format!(
            "{ended_run}    review_runs:\n    - id: R1-review1\n      agent: codex\n      worktree: /tmp/w\n      branch: corral/T-009\n      base: main\n      base_sha: '00'\n      started: 2026-10-04T00:30:00Z\n"
        ),
        "open_plan_run" => "  plan_runs:\n  - id: D1\n    profile: p\n    harness: h\n    model: null\n    vendor: null\n    effort: null\n    spec_revision: s\n    started: 2026-10-04T00:00:00Z\n    ended: null\n    herdr: null\n    output_dir: /tmp/o\n    submitted: null\n    rounds: 0\n    state: planning\n    pending_reply: null\n".to_owned(),
        "launch_reservation" => "  launch:\n    stage: reserved\n    reserved: true\n".to_owned(),
        "review_reservation" => format!("{ended_run}  review_reservation: gate\n"),
        "live_gate" => format!(
            "{ended_run}    gate:\n      result: running\n      at: 2026-10-04T00:40:00Z\n      sha: '22'\n      pane: w9:p9\n"
        ),
        other => panic!("unknown family {other}"),
    };
    fs::write(
        tasks.join("T-009-injected.md"),
        format!("{head}{body}---\nBody\n"),
    )
    .unwrap();
}

#[test]
fn apply_refuses_each_blocker_family_and_changes_nothing() {
    for family in [
        "open_coding_run",
        "open_review_run",
        "open_plan_run",
        "launch_reservation",
        "review_reservation",
        "live_gate",
    ] {
        // Given a drained board with one blocker of this family injected.
        let legacy = Legacy::new();
        inject(&legacy, family);
        let before = legacy.legacy_bytes();
        // When an apply runs.
        let error = legacy.migrate().apply().unwrap_err();
        // Then it is refused naming the blocker, with no journal, fence or file change.
        assert!(format!("{error:#}").contains(family), "{family}: {error:#}");
        assert!(
            format!("{error:#}").contains("T-009"),
            "{family}: {error:#}"
        );
        assert!(legacy.journal().is_none(), "{family}");
        assert_eq!(legacy.fence().as_deref(), Some(FENCE), "{family}");
        assert_eq!(legacy.legacy_bytes(), before, "{family}");
        legacy.invariant();
    }
}

#[test]
fn apply_publishes_a_database_equal_to_the_legacy_store() {
    // Given a drained legacy board.
    let legacy = Legacy::new();
    let before = legacy.legacy_bytes();
    // When it is applied.
    let applied = legacy.migrate().apply().unwrap();
    // Then the journal is published at the import's change count, with fence 4 and only db.
    assert_eq!(applied.state, "published");
    let journal = legacy.journal().unwrap();
    assert_eq!(journal["state"], "published");
    assert_eq!(legacy.fence().as_deref(), Some("4\n"));
    assert!(legacy.exists("db") && !legacy.exists("db.migrating") && !legacy.exists("db.discard"));
    assert_eq!(journal["change_seq_at_publish"], legacy.integrity("db"));
    legacy.invariant();
    // And the legacy tree is untouched, and the v2 store opens with every task equal.
    assert_eq!(legacy.legacy_bytes(), before);
    let store = corral_core::Store::open(&legacy.root).unwrap();
    let tasks = store.list().unwrap();
    assert_eq!(tasks.len(), 3);
    let done = store.get("T-001").unwrap();
    assert!(done.runs[0].gate.as_ref().unwrap().process_lost);
    assert_eq!(done.artifacts.len(), 1);
    assert_eq!(store.events().unwrap().len(), 5);
    let import = applied.import.unwrap();
    assert_eq!((import.tasks, import.events), (3, 5));
    // And a second apply is a no-op.
    drop(store);
    assert_eq!(legacy.migrate().apply().unwrap().resumed_from, "published");
}

#[test]
fn every_forward_barrier_resumes_to_published() {
    for at in ["B1", "B2", "B3", "B4", "B5", "B6", "B7"] {
        // Given a legacy board and an apply killed at the barrier.
        let legacy = Legacy::new();
        let before = legacy.legacy_bytes();
        let (barrier, _) = kill_at(at);
        killed(legacy.migrate().barrier(barrier).apply(), at);
        // Then status names a state and changes nothing, and the invariant holds.
        let status = legacy.migrate().status().unwrap();
        assert!(status.problems.is_empty(), "{at}: {status:?}");
        assert_eq!(legacy.migrate().status().unwrap().state, status.state);
        legacy.invariant();
        assert_eq!(legacy.legacy_bytes(), before, "{at}");
        let expected_state = match at {
            "B1" | "B2" => "fenced",
            "B3" => "importing",
            "B4" | "B5" | "B6" => "verified",
            _ => "published",
        };
        assert_eq!(status.state, expected_state, "{at}");
        if at == "B6" {
            assert_eq!(status.candidate.as_deref(), Some("db"), "{at}");
        } else if at == "B4" || at == "B5" {
            assert_eq!(status.candidate.as_deref(), Some("db.migrating"), "{at}");
        }
        // When the apply reruns, it publishes.
        let applied = legacy.migrate().apply().unwrap();
        assert_eq!(applied.state, "published", "{at}");
        assert_eq!(legacy.fence().as_deref(), Some("4\n"), "{at}");
        legacy.integrity("db");
        legacy.invariant();
        assert_eq!(legacy.legacy_bytes(), before, "{at}");
    }
}

#[test]
fn apply_refuses_database_directories_without_a_journal() {
    for name in ["db", "db.migrating", "db.discard"] {
        // Given a legacy board with a stray database directory and no journal (I6).
        let legacy = Legacy::new();
        fs::create_dir(legacy.corral.join(name)).unwrap();
        // When an apply runs, it refuses naming the path; nothing changes.
        let error = legacy.migrate().apply().unwrap_err();
        assert!(format!("{error:#}").contains(name), "{error:#}");
        assert!(legacy.journal().is_none());
        assert_eq!(legacy.fence().as_deref(), Some(FENCE));
        assert!(legacy.exists(name));
    }
}

#[test]
fn a_verified_journal_with_both_or_neither_candidate_refuses_to_publish() {
    // Given an apply killed after verification (B4: candidate at db.migrating).
    let legacy = Legacy::new();
    let (barrier, _) = kill_at("B4");
    killed(legacy.migrate().barrier(barrier).apply(), "B4");
    // When db also appears, the apply refuses and leaves both.
    fs::create_dir(legacy.corral.join("db")).unwrap();
    let error = legacy.migrate().apply().unwrap_err();
    assert!(format!("{error:#}").contains("db"), "{error:#}");
    assert!(legacy.exists("db") && legacy.exists("db.migrating"));
    assert_eq!(
        legacy.journal().unwrap()["state"],
        JournalState::Verified.as_str()
    );
}

#[test]
fn apply_refuses_a_same_size_same_mtime_content_change_during_the_import() {
    // Given an import during which a stored review note changes after its blob was taken,
    // keeping its size and mtime.
    let legacy = Legacy::new();
    let corral = legacy.corral.clone();
    let note = "state/reviews/T-001-R1-notes.md";
    let barrier = support::act_at("B3", move || {
        support::alter_in_place(&corral, note, "good", "fine");
        Ok(())
    });
    // When the apply compares the legacy files with M0 (A5), it refuses naming the file.
    let error = legacy.migrate().barrier(barrier).apply().unwrap_err();
    assert!(
        format!("{error:#}").contains(&format!("{note}: content changed")),
        "{error:#}"
    );
    // Then nothing is verified or published.
    assert_eq!(legacy.journal().unwrap()["state"], "importing");
    assert!(!legacy.exists("db"));
    assert_eq!(legacy.fence().as_deref(), Some("4\n"));
}

/// Writes T-009 (in the backlog) with `extra` merged into its task document.
fn injected_task(legacy: &Legacy, extra: serde_json::Value) {
    let path = legacy.corral.join("tasks/T-009-injected.md");
    let task = corral_core::parse_task_record(&support::simple_task("T-009", "backlog"), &path)
        .unwrap()
        .task;
    let mut doc = serde_json::to_value(&task).unwrap();
    for (key, value) in extra.as_object().unwrap() {
        doc[key] = value.clone();
    }
    let task: corral_core::Task = serde_json::from_value(doc).unwrap();
    fs::write(&path, corral_core::render_task(&task)).unwrap();
}

fn identity(task: &str) -> serde_json::Value {
    serde_json::json!({
        "version": 2, "task": task, "run": "aggregate", "attempt": "ag-1", "head": "1".repeat(40),
        "tree": "2".repeat(40), "integration_base": "3".repeat(40), "dispatch_base": "4".repeat(40),
        "spec_digest": "5".repeat(64), "policy_digest": "6".repeat(64),
        "inputs_digest": "7".repeat(64), "environment": []
    })
}

/// Injects one blocker of `family` and returns the task its blocker names.
fn inject_state(legacy: &Legacy, family: &str) -> &'static str {
    use serde_json::json;
    let support = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/migrate_support");
    match family {
        "pending_plan_review" => injected_task(
            legacy,
            json!({"plans": [{
                "id": "P1", "spec_revision": "s",
                "artifact": {"run": "D1", "kind": "plan", "title": "Plan", "path": "artifacts/T-009/D1/p.md",
                    "source": "/tmp/p.md", "sha256": "0".repeat(64), "bytes": 0,
                    "added": "2026-10-04T00:00:00Z", "purged_at": "2026-10-04T00:00:00Z"},
                "document": {"goal": "g", "non_goals": [], "decisions": [], "affected_paths": [],
                    "contracts": [], "dependencies": [], "acceptance": [], "verification": [],
                    "owner_decisions": []},
                "reviews": [{"id": "P1-review1", "reviewer": "codex", "policy_digest": "d",
                    "at": "2026-10-04T00:10:00Z"}]
            }]}),
        ),
        "pending_aggregate_review" => {
            let route: serde_json::Value =
                serde_json::from_slice(&fs::read(support.join("review-route.json")).unwrap())
                    .unwrap();
            injected_task(
                legacy,
                json!({"aggregate_attempts": [{
                    "id": "ag-1", "identity": identity("T-009"), "target": {"kind": "main"},
                    "code": false, "members": [], "started": "2026-10-04T00:00:00Z",
                    "reviews": [{"id": "ag-1-review1", "reviewer": "codex", "route": route,
                        "identity": identity("T-009"), "evidence_digest": "e",
                        "requested": "2026-10-04T00:10:00Z"}]
                }]}),
            );
        }
        "review_wait" => injected_task(legacy, json!({"review_wait": "gate"})),
        "submitting_authorization" => {
            let bytes = fs::read(support.join("submitting-authorization.json")).unwrap();
            let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let id = doc["authorization"]["id"].as_str().unwrap();
            legacy.put(&format!("state/action-authorizations/{id}.json"), &bytes);
        }
        "prepared_review_journal" => {
            injected_task(legacy, json!({}));
            legacy.put(
                "state/review-prepared/T-009-R1.json",
                br#"{"task":"T-009","run":"R1"}"#,
            );
        }
        "prepared_plan_review_journal" => {
            legacy.put(
                "state/plan-review-T-010-P1.json",
                br#"{"plan":"P1","review":{"id":"P1-review1","verdict":null}}"#,
            );
            return "T-010";
        }
        "review_launch_journal" => legacy.put(
            "state/review-launch.json",
            br#"{"task":"T-009","run":"R1","pane":"w9:p9"}"#,
        ),
        other => panic!("unknown family {other}"),
    }
    "T-009"
}

#[test]
fn apply_refuses_the_journal_review_and_authorization_families_and_changes_nothing() {
    for family in [
        "pending_plan_review",
        "pending_aggregate_review",
        "review_wait",
        "submitting_authorization",
        "prepared_review_journal",
        "prepared_plan_review_journal",
        "review_launch_journal",
    ] {
        // Given a drained board with one blocker of this family injected.
        let legacy = Legacy::new();
        let task = inject_state(&legacy, family);
        let before = legacy.legacy_bytes();
        // When the dry run reports and an apply runs.
        let dry = legacy.migrate().dry_run().unwrap();
        let error = format!("{:#}", legacy.migrate().apply().unwrap_err());
        // Then both name the blocker and its task, with no journal, fence or file change.
        assert!(
            dry.blockers
                .iter()
                .any(|b| b.family == family && b.task == task),
            "{family}: {:?}",
            dry.blockers
        );
        assert!(
            error.contains(family) && error.contains(task),
            "{family}: {error}"
        );
        assert!(legacy.journal().is_none(), "{family}");
        assert_eq!(legacy.fence().as_deref(), Some(FENCE), "{family}");
        assert_eq!(legacy.legacy_bytes(), before, "{family}");
        legacy.invariant();
    }
}

/// Every task's header fields (without `updated`), summary and rows in `corral`'s database.
fn task_rows(
    corral: &std::path::Path,
) -> std::collections::BTreeMap<String, (serde_json::Value, serde_json::Value, Vec<HistoryRow>)> {
    use corral_core::backend::{StoreBackend, TaskNum, TaskParts};
    let backend = corral_core::sqlite::SqliteBackend::open(
        corral,
        corral_core::sqlite::SqliteOptions::default(),
    )
    .unwrap();
    let view = backend.view().unwrap();
    let mut out = std::collections::BTreeMap::new();
    for (id, num) in [
        ("T-001", 1),
        ("T-002", 2),
        ("T-003", 3),
        ("T-072", 72),
        ("T-227", 227),
    ] {
        let doc = view.task(TaskNum(num), &TaskParts::All).unwrap().unwrap();
        let mut fields = serde_json::to_value(&doc.head.header.fields).unwrap();
        fields.as_object_mut().unwrap().remove("updated");
        let mut rows = doc.rows;
        rows.sort_by(|a, b| (a.part, &a.key).cmp(&(b.part, &b.key)));
        out.insert(id.to_owned(), (fields, doc.head.summary, rows));
    }
    out
}

#[test]
fn imported_rows_equal_a_v2_write_and_add_only_the_event_only_panes() {
    use corral_core::backend::{EventQuery, HeadQuery, PageReq, StoreBackend, TaskPart};
    // Given the fixture board plus two real legacy tasks (T-072 with a retried run, so its
    // first run is history; T-227 with a changes review), each with a dashboard schedule.
    let legacy = Legacy::new();
    legacy.add_real_task("T-072-legacy.md");
    legacy.add_real_task("T-227-legacy.md");
    let path = legacy.corral.join("tasks/T-072-legacy.md");
    let mut retried = corral_core::parse_task_record(&fs::read_to_string(&path).unwrap(), &path)
        .unwrap()
        .task;
    let mut again = retried.runs[0].clone();
    again.id = "R2".to_owned();
    retried.runs.push(again);
    fs::write(&path, corral_core::render_task(&retried)).unwrap();
    let records: Vec<corral_core::Task> = [
        "T-001-finished-work.md",
        "T-002-backlog.md",
        "T-003-flat.md",
        "T-072-legacy.md",
        "T-227-legacy.md",
    ]
    .iter()
    .map(|file| {
        let path = legacy.corral.join("tasks").join(file);
        corral_core::parse_task_record(&fs::read_to_string(&path).unwrap(), &path)
            .unwrap()
            .task
    })
    .collect();
    // When it is migrated, and the same records are saved by a fresh v2 store's ordinary write.
    legacy.migrate().apply().unwrap();
    let fresh = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(fresh.path()).unwrap();
    for task in &records {
        store.save(task).unwrap();
    }
    drop(store);
    let migrated = task_rows(&legacy.corral);
    let written = task_rows(&fresh.path().join(".corral"));
    // Then every header field, summary and derived row equals the ordinary write's.
    let derived = |rows: &[HistoryRow]| -> Vec<HistoryRow> {
        rows.iter()
            .filter(|r| !matches!(r.part, TaskPart::CleanupPane | TaskPart::GateAttempt))
            .cloned()
            .collect()
    };
    for (id, (fields, summary, rows)) in &migrated {
        let (written_fields, written_summary, written_rows) = &written[id];
        assert_eq!(fields, written_fields, "{id} header fields");
        assert_eq!(summary, written_summary, "{id} summary");
        assert_eq!(derived(rows), derived(written_rows), "{id} rows");
        assert!(!written_rows.iter().any(|r| r.part == TaskPart::GateAttempt));
        // And the migration adds only what the ordinary write never derives: one gate attempt
        // per gate identity a run records, and the panes only legacy events name (T-001 R1's
        // gate pane from gate_started and fallback workspace from run_workspace_created).
        let gates: Vec<String> = records
            .iter()
            .find(|t| &t.id == id)
            .unwrap()
            .runs
            .iter()
            .filter_map(|run| {
                let identity = run.gate.as_ref()?.identity.as_ref()?;
                Some(format!("{}/{}", run.id, identity.attempt))
            })
            .collect();
        let imported: Vec<String> = rows
            .iter()
            .filter(|r| r.part == TaskPart::GateAttempt)
            .map(|r| r.key.clone())
            .collect();
        assert_eq!(imported, gates, "{id} gate attempts");
        let panes: Vec<(String, serde_json::Value)> = rows
            .iter()
            .filter(|r| r.part == TaskPart::CleanupPane && !written_rows.contains(r))
            .map(|r| (r.key.clone(), r.doc.clone()))
            .collect();
        let expected: Vec<(String, serde_json::Value)> = if id == "T-001" {
            vec![
                (
                    "run:000001/000000/gate/w1:p3".to_owned(),
                    serde_json::json!({"kind": "gate", "pane": "w1:p3", "socket": "/tmp/h.sock", "workspace": null}),
                ),
                (
                    "run:000001/000000/workspace/w2:p1".to_owned(),
                    serde_json::json!({"kind": "workspace", "pane": "w2:p1", "socket": "", "workspace": "w2"}),
                ),
            ]
        } else {
            Vec::new()
        };
        assert_eq!(panes, expected, "{id} event-only panes");
    }
    // And the real tasks' separately derived rows are populated: usage, switches, review
    // children, a finding, and checkpoint metas each naming a stored payload.
    let count = |id: &str, part: TaskPart| migrated[id].2.iter().filter(|r| r.part == part).count();
    assert_eq!(
        [
            TaskPart::Run,
            TaskPart::RunUsage,
            TaskPart::RunSwitch,
            TaskPart::ReviewRun,
            TaskPart::RunCheckpoint,
            TaskPart::CheckpointPayload
        ]
        .map(|part| count("T-072", part)),
        [2, 118, 2, 2, 2, 1]
    );
    assert_eq!(
        [
            TaskPart::Finding,
            TaskPart::ReviewRun,
            TaskPart::RunUsage,
            TaskPart::RunCheckpoint,
            TaskPart::CheckpointPayload
        ]
        .map(|part| count("T-227", part)),
        [1, 2, 88, 2, 2]
    );
    for id in ["T-072", "T-227"] {
        let rows = &migrated[id].2;
        for meta in rows.iter().filter(|r| r.part == TaskPart::RunCheckpoint) {
            let digest = meta.doc["payload_digest"].as_str().unwrap();
            assert!(
                rows.iter()
                    .any(|r| r.part == TaskPart::CheckpointPayload && r.key == digest),
                "{id} checkpoint {} names a missing payload",
                meta.key
            );
        }
    }
    // And the HistorySummary counts T-072's earlier run, which only the history holds.
    assert!(!migrated["T-072"].1["last_ended_run"].is_null());
    // And the indexes outside the task document: dashboard_due serves the due-schedule query,
    // and the events' run keys serve a run-scoped event read.
    assert_eq!(
        migrated["T-072"].0["dashboard_due"],
        serde_json::json!("2026-10-01T00:00:00Z")
    );
    let backend = corral_core::sqlite::SqliteBackend::open(
        &legacy.corral,
        corral_core::sqlite::SqliteOptions::default(),
    )
    .unwrap();
    let view = backend.view().unwrap();
    let due: Vec<String> = view
        .heads(
            &HeadQuery::DashboardDue {
                now: "2026-10-02T00:00:00Z".parse().unwrap(),
            },
            PageReq::first(10),
        )
        .unwrap()
        .items
        .into_iter()
        .map(|head| head.header.id)
        .collect();
    assert_eq!(due, ["T-072", "T-227"]);
    let query = EventQuery {
        after: None,
        task: Some("T-001".to_owned()),
        kinds: Vec::new(),
        run: Some("R1".to_owned()),
        newest_first: false,
        valid_only: true,
    };
    let kinds: Vec<String> = view
        .events(&query, PageReq::first(10))
        .unwrap()
        .items
        .into_iter()
        .map(|e| e.event.kind)
        .collect();
    assert_eq!(
        kinds,
        ["dispatched", "gate_started", "run_workspace_created"]
    );
    drop(view);
    drop(backend);
    // And the T-252 run state survives on the assembled tasks: agent state and the run inbox's
    // observe cursor.
    let store = corral_core::Store::open(&legacy.root).unwrap();
    for id in ["T-072", "T-227"] {
        let task = store.get(id).unwrap();
        assert!(
            task.runs.iter().all(|run| run.agent_state.is_some()),
            "{id}"
        );
    }
    let t227 = store.get("T-227").unwrap();
    assert!(serde_json::to_value(&t227.runs[0]).unwrap()["inbox"]["observe_cursor"].is_object());
}
