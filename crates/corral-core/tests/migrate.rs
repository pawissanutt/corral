//! `corral migrate` forward path: dry run, the drain check, A1-A7 with the import checks, the
//! forward barriers B1-B7 and the injected states I5-I6 (plan T-253 section 12).

#[path = "migrate_support/mod.rs"]
mod support;

use std::fs;

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
