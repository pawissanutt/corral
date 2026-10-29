//! `corral migrate --rollback` and `--finalize` (plan T-253 section 12): published rollback
//! R0-R7 with barriers B8-B14, pre-publish rollback U0-U6 with B15-B17, the composed
//! sequences B20-B26, finalize with B18-B19, and the injected states I1-I5.

#[path = "migrate_support/mod.rs"]
mod support;

use std::fs;

use support::{FENCE, Legacy, kill_at, killed};

fn rolled_back(legacy: &Legacy, before: &std::collections::BTreeMap<String, Vec<u8>>) {
    assert_eq!(legacy.fence().as_deref(), Some(FENCE));
    assert!(legacy.journal().is_none());
    for name in ["db", "db.migrating", "db.discard"] {
        assert!(!legacy.exists(name), "{name} remains");
    }
    assert_eq!(&legacy.legacy_bytes(), before);
    legacy.invariant();
    let error = corral_core::Store::open(&legacy.root).unwrap_err();
    assert!(format!("{error:#}").contains("corral migrate"), "{error:#}");
}

fn archive(legacy: &Legacy) -> String {
    let entries: Vec<_> = fs::read_dir(legacy.corral.join("rollback"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    format!("rollback/{}", entries[0])
}

#[test]
fn published_rollback_archives_the_database_and_restores_the_fence() {
    // Given a published store with no v2 changes.
    let legacy = Legacy::new();
    let before = legacy.legacy_bytes();
    legacy.migrate().apply().unwrap();
    let seq = legacy.integrity("db");
    // When it is rolled back.
    let done = legacy.migrate().rollback(None).unwrap();
    // Then the legacy store is back and the whole database is archived intact.
    rolled_back(&legacy, &before);
    let archive = archive(&legacy);
    assert_eq!(done.archive.as_deref(), Some(archive.as_str()));
    assert_eq!(legacy.integrity(&format!("{archive}/db")), seq);
    assert!(legacy.exists(&format!("{archive}/ROLLBACK.json")));
}

#[test]
fn published_rollback_refuses_unnamed_v2_changes_and_archives_named_ones() {
    // Given a published store with one v2 write.
    let legacy = Legacy::new();
    let before = legacy.legacy_bytes();
    legacy.migrate().apply().unwrap();
    let store = corral_core::Store::open(&legacy.root).unwrap();
    store
        .create("v2 only", None, corral_core::Status::Backlog, "")
        .unwrap();
    drop(store);
    let n = legacy.migrate().status().unwrap().changes.unwrap();
    assert!(n > 0);
    // When rollback runs without, and with a wrong, discard count, it refuses naming N.
    for discard in [None, Some(n + 1)] {
        let error = legacy.migrate().rollback(discard).unwrap_err();
        assert!(format!("{error:#}").contains(&n.to_string()), "{error:#}");
        assert_eq!(legacy.journal().unwrap()["state"], "published");
        assert_eq!(legacy.fence().as_deref(), Some("4\n"));
    }
    // Then naming exactly N rolls back and keeps the write in the archive.
    let done = legacy.migrate().rollback(Some(n)).unwrap();
    assert_eq!(done.discarded_changes, Some(n));
    rolled_back(&legacy, &before);
    legacy.integrity(&format!("{}/db", archive(&legacy)));
}

#[test]
fn every_published_rollback_barrier_resumes_to_the_legacy_store() {
    for (at, state, step, fence) in [
        ("B8", "published", None, "4\n"),
        ("B9", "rolling_back", Some("intent"), "4\n"),
        ("B10", "rolling_back", Some("intent"), "4\n"),
        ("B11", "rolling_back", Some("archived"), "4\n"),
        ("B14", "rolling_back", Some("archived"), "4\n"),
        ("B12", "rolling_back", Some("archived"), FENCE),
        ("B13", "rolling_back", Some("fence_restored"), FENCE),
    ] {
        // Given a published store and a rollback killed at the barrier.
        let legacy = Legacy::new();
        let before = legacy.legacy_bytes();
        legacy.migrate().apply().unwrap();
        let seq = legacy.integrity("db");
        let (barrier, _) = kill_at(at);
        killed(legacy.migrate().barrier(barrier).rollback(None), at);
        // Then status names the state and step and changes nothing; the fence is whole.
        let status = legacy.migrate().status().unwrap();
        assert_eq!(
            (status.state.as_str(), status.step.as_deref()),
            (state, step),
            "{at}"
        );
        assert_eq!(legacy.fence().as_deref(), Some(fence), "{at}");
        assert!(status.problems.is_empty(), "{at}: {status:?}");
        legacy.invariant();
        if state == "rolling_back" {
            let error = corral_core::Store::open(&legacy.root).unwrap_err();
            assert!(
                format!("{error:#}").contains("--rollback"),
                "{at}: {error:#}"
            );
            let error = legacy.migrate().apply().unwrap_err();
            assert!(
                format!("{error:#}").contains("finish the rollback"),
                "{at}: {error:#}"
            );
        }
        // And the database sits whole at db or in the archive with an unchanged change count.
        let at_db = if legacy.exists("db") {
            "db".to_owned()
        } else {
            format!("{}/db", archive(&legacy))
        };
        assert_eq!(legacy.integrity(&at_db), seq, "{at}");
        // When the rollback reruns, the legacy store is restored.
        legacy.migrate().rollback(None).unwrap();
        rolled_back(&legacy, &before);
    }
}

#[test]
fn every_pre_publish_rollback_barrier_discards_a_partial_import() {
    for (at, step) in [("B15", "intent"), ("B16", "intent"), ("B17", "quarantined")] {
        // Given an apply killed mid-import (B3) and its rollback killed at the barrier.
        let legacy = Legacy::new();
        let before = legacy.legacy_bytes();
        let (barrier, _) = kill_at("B3");
        killed(legacy.migrate().barrier(barrier).apply(), "B3");
        let (barrier, _) = kill_at(at);
        killed(legacy.migrate().barrier(barrier).rollback(None), at);
        // Then status names the step and the candidate, and the invariant holds.
        let status = legacy.migrate().status().unwrap();
        assert_eq!(status.step.as_deref(), Some(step), "{at}");
        assert_eq!(status.candidate.as_deref(), Some("db.migrating"), "{at}");
        assert!(status.problems.is_empty(), "{at}: {status:?}");
        assert_eq!(legacy.fence().as_deref(), Some("4\n"));
        legacy.invariant();
        // When it reruns, the candidate is gone and the legacy store is back.
        legacy.migrate().rollback(None).unwrap();
        rolled_back(&legacy, &before);
    }
}

/// The composed tail of B20-B26: the restored legacy store takes a 0.3.x write, re-apply
/// publishes from it, and storage checks.
fn composed_tail(legacy: &Legacy) {
    legacy.legacy_comment("T-002-backlog.md", "written by 0.3.x after the rollback");
    legacy.migrate().apply().unwrap();
    assert!(legacy.exists("db") && !legacy.exists("db.migrating") && !legacy.exists("db.discard"));
    let store = corral_core::Store::open(&legacy.root).unwrap();
    assert!(store.check_database().unwrap().is_empty());
    let task = store.get("T-002").unwrap();
    assert_eq!(task.comments.len(), 1);
    assert_eq!(task.comments[0].text, "written by 0.3.x after the rollback");
}

#[test]
fn rollback_from_verified_discards_the_candidate_from_either_location() {
    for (kill, candidate) in [("B6", "db"), ("B4", "db.migrating")] {
        // Given an apply killed after verification (B20: at db; B26: at db.migrating).
        let legacy = Legacy::new();
        let before = legacy.legacy_bytes();
        let (barrier, _) = kill_at(kill);
        killed(legacy.migrate().barrier(barrier).apply(), kill);
        let status = legacy.migrate().status().unwrap();
        assert_eq!(status.state, "verified");
        assert_eq!(status.candidate.as_deref(), Some(candidate));
        assert!(status.next.iter().any(|n| n.contains("--rollback")));
        // When it is rolled back, then the candidate is discarded and the fence restored.
        let done = legacy.migrate().rollback(None).unwrap();
        assert_eq!(done.candidate.as_deref(), Some(candidate));
        rolled_back(&legacy, &before);
        composed_tail(&legacy);
    }
}

#[test]
fn a_rollback_from_verified_killed_again_still_reaches_the_legacy_store() {
    for again in ["B15", "B16", "B17", "B24", "B12"] {
        // Given an apply killed after A6's rename and its rollback killed again (B21-B25).
        let legacy = Legacy::new();
        let before = legacy.legacy_bytes();
        let (barrier, _) = kill_at("B6");
        killed(legacy.migrate().barrier(barrier).apply(), "B6");
        let (barrier, _) = kill_at(again);
        killed(legacy.migrate().barrier(barrier).rollback(None), again);
        legacy.invariant();
        let status = legacy.migrate().status().unwrap();
        assert!(status.problems.is_empty(), "{again}: {status:?}");
        assert_eq!(status.candidate.as_deref(), Some("db"), "{again}");
        // When the rollback reruns, it reaches B20's end state and the composed tail holds.
        legacy.migrate().rollback(None).unwrap();
        rolled_back(&legacy, &before);
        composed_tail(&legacy);
    }
}

#[test]
fn rollback_before_any_candidate_restores_the_fence() {
    for kill in ["B1", "B2"] {
        // Given an apply killed while fenced (B1: old fence; B2: fence 4).
        let legacy = Legacy::new();
        let before = legacy.legacy_bytes();
        let (barrier, _) = kill_at(kill);
        killed(legacy.migrate().barrier(barrier).apply(), kill);
        // When it is rolled back, then the legacy store is unchanged.
        let done = legacy.migrate().rollback(None).unwrap();
        assert_eq!(done.candidate.as_deref(), Some("none"));
        rolled_back(&legacy, &before);
    }
}

#[test]
fn inconsistent_pre_publish_states_refuse_rollback_and_change_nothing() {
    type Inject = fn(&Legacy);
    let cases: [(&str, &str, Inject); 7] = [
        ("B4", "db", |l| fs::create_dir(l.corral.join("db")).unwrap()),
        ("B4", "db.migrating", |l| {
            fs::remove_dir_all(l.corral.join("db.migrating")).unwrap()
        }),
        ("B4", "migration_digest", |l| {
            edit_journal(l, "migration_digest", "0".repeat(64).into())
        }),
        ("B4", "change_seq", |l| {
            let seq = l.journal().unwrap()["import_seq"].as_u64().unwrap();
            edit_journal(l, "import_seq", (seq + 1).into());
        }),
        ("B4", "db.discard", |l| {
            fs::create_dir(l.corral.join("db.discard")).unwrap()
        }),
        ("B1", "db", |l| fs::create_dir(l.corral.join("db")).unwrap()),
        ("B3", "db", |l| {
            std::os::unix::fs::symlink("/tmp", l.corral.join("db")).unwrap()
        }),
    ];
    for (kill, named, inject) in cases {
        // Given a pre-publish journal with an injected inconsistency (I1-I5).
        let legacy = Legacy::new();
        let (barrier, _) = kill_at(kill);
        killed(legacy.migrate().barrier(barrier).apply(), kill);
        inject(&legacy);
        let journal = legacy.journal();
        let fence = legacy.fence();
        let dirs: Vec<bool> = ["db", "db.migrating", "db.discard"]
            .map(|d| legacy.exists(d))
            .to_vec();
        // When rollback runs, it refuses naming the path or value and changes nothing.
        let error = legacy.migrate().rollback(None).unwrap_err();
        assert!(
            format!("{error:#}").contains(named),
            "{kill} {named}: {error:#}"
        );
        assert_eq!(legacy.journal(), journal, "{kill} {named}");
        assert_eq!(legacy.fence(), fence, "{kill} {named}");
        let after: Vec<bool> = ["db", "db.migrating", "db.discard"]
            .map(|d| legacy.exists(d))
            .to_vec();
        assert_eq!(after, dirs, "{kill} {named}");
    }
}

fn edit_journal(legacy: &Legacy, field: &str, value: serde_json::Value) {
    let mut journal = legacy.journal().unwrap();
    journal[field] = value;
    fs::write(
        legacy.corral.join("migration.json"),
        serde_json::to_string_pretty(&journal).unwrap(),
    )
    .unwrap();
}

#[test]
fn finalize_deletes_legacy_paths_and_ends_the_rollback_window() {
    for kill in [None, Some("B18"), Some("B19")] {
        // Given a published store, finalized (killed at the barrier and rerun).
        let legacy = Legacy::new();
        fs::create_dir_all(legacy.corral.join("lessons")).unwrap();
        fs::write(legacy.corral.join("lessons/corral.md"), "lessons\n").unwrap();
        legacy.migrate().apply().unwrap();
        if let Some(at) = kill {
            let (barrier, _) = kill_at(at);
            killed(legacy.migrate().barrier(barrier).finalize(), at);
            assert_eq!(legacy.journal().unwrap()["state"], "finalizing", "{at}");
            let error = legacy.migrate().rollback(None).unwrap_err();
            assert!(format!("{error:#}").contains("finalize"), "{at}: {error:#}");
            corral_core::Store::open(&legacy.root).unwrap();
        }
        let done = legacy.migrate().finalize().unwrap();
        // Then the legacy tree is gone, the fence, lessons and database stay, v2 opens,
        // and rollback is refused.
        assert_eq!(done.state, "finalized");
        for gone in [
            "tasks",
            "events.jsonl",
            "artifacts",
            "state/pm.json",
            "state/reviews",
        ] {
            assert!(!legacy.exists(gone), "{gone} remains");
        }
        assert_eq!(legacy.fence().as_deref(), Some("4\n"));
        assert!(legacy.exists("lessons/corral.md") && legacy.exists("db"));
        let store = corral_core::Store::open(&legacy.root).unwrap();
        assert_eq!(store.list().unwrap().len(), 3);
        drop(store);
        let error = legacy.migrate().rollback(None).unwrap_err();
        assert!(format!("{error:#}").contains("finalize"), "{error:#}");
        assert_eq!(legacy.migrate().finalize().unwrap().state, "finalized");
    }
}

fn hold_gc_claim(legacy: &Legacy, owner: &str, doc: serde_json::Value) {
    use corral_core::backend::{Claim, Operation, STORAGE_GC_CLAIM, StoreBackend};
    let backend = corral_core::sqlite::SqliteBackend::open(
        &legacy.corral,
        corral_core::sqlite::SqliteOptions::default(),
    )
    .unwrap();
    let claim = Operation::Claim(Claim {
        name: STORAGE_GC_CLAIM.to_owned(),
        owner: owner.to_owned(),
        acquired: jiff::Timestamp::now(),
        doc,
        reservations: Vec::new(),
    });
    backend.apply(claim).unwrap();
}

fn release_gc_claim(legacy: &Legacy, owner: &str) {
    use corral_core::backend::{Operation, ReleaseClaim, STORAGE_GC_CLAIM, StoreBackend};
    let backend = corral_core::sqlite::SqliteBackend::open(
        &legacy.corral,
        corral_core::sqlite::SqliteOptions::default(),
    )
    .unwrap();
    let release = Operation::ReleaseClaim(ReleaseClaim {
        name: STORAGE_GC_CLAIM.to_owned(),
        owner: owner.to_owned(),
    });
    backend.apply(release).unwrap();
}

#[test]
fn finalize_takes_over_a_dead_holders_storage_gc_claim_and_refuses_a_live_one() {
    use corral_core::blob::ProcessIdentity;
    // Given a published store whose storage-gc claim an unrecognized document holds.
    let legacy = Legacy::new();
    legacy.migrate().apply().unwrap();
    hold_gc_claim(&legacy, "someone", serde_json::json!({"pid": 1}));
    // When finalize runs, it refuses naming the holder; the store stays finalizing.
    let error = legacy.migrate().finalize().unwrap_err();
    assert!(format!("{error:#}").contains("someone"), "{error:#}");
    assert_eq!(legacy.journal().unwrap()["state"], "finalizing");
    release_gc_claim(&legacy, "someone");
    // Given a live process holding the claim (a running storage gc).
    let mut gc = std::process::Command::new("sleep")
        .arg("600")
        .stdin(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let holder = ProcessIdentity::of(gc.id()).unwrap().unwrap();
    hold_gc_claim(
        &legacy,
        &holder.owner(),
        serde_json::to_value(&holder).unwrap(),
    );
    // When finalize runs while it lives, it refuses naming that holder and deletes nothing.
    let error = legacy.migrate().finalize().unwrap_err();
    assert!(format!("{error:#}").contains(&holder.owner()), "{error:#}");
    assert!(legacy.exists("tasks") && legacy.exists("events.jsonl"));
    // When that process dies holding the claim, the rerun takes the claim over and finalizes.
    gc.kill().unwrap();
    gc.wait().unwrap();
    let done = legacy.migrate().finalize().unwrap();
    assert_eq!(done.state, "finalized");
    assert!(!legacy.exists("tasks"));
}

#[test]
fn reapply_after_rollback_deduplicates_blobs_and_reports_reclaimed_build_output() {
    // Given a published store whose legacy build output storage gc reclaimed.
    let legacy = Legacy::new();
    let first = legacy.migrate().apply().unwrap().import.unwrap();
    fs::remove_dir_all(legacy.corral.join("state/reviews/T-001-R1-probe/target")).unwrap();
    // When it is rolled back, the reclaimed build output is reported, not refused.
    let done = legacy.migrate().rollback(None).unwrap();
    assert_eq!(done.reclaimed_build_output.len(), 1);
    assert_eq!(
        done.reclaimed_build_output[0].path,
        "state/reviews/T-001-R1-probe/target"
    );
    // Then a re-apply publishes again, reusing every blob the first import stored.
    let second = legacy.migrate().apply().unwrap().import.unwrap();
    assert_eq!(second.blobs, first.blobs);
    assert_eq!(second.blobs_reused, first.blobs);
}

#[test]
fn rollback_refuses_a_same_size_same_mtime_content_change() {
    // Given a published store whose legacy task file changed, keeping its size and mtime.
    let legacy = Legacy::new();
    legacy.migrate().apply().unwrap();
    support::alter_in_place(
        &legacy.corral,
        "tasks/T-002-backlog.md",
        "Body of",
        "Text of",
    );
    // When it is rolled back, the M0 comparison (R1) refuses naming the file.
    let error = legacy.migrate().rollback(None).unwrap_err();
    assert!(
        format!("{error:#}").contains("tasks/T-002-backlog.md: content changed"),
        "{error:#}"
    );
    // Then nothing changed: still published at fence 4 with the database in place.
    assert_eq!(legacy.journal().unwrap()["state"], "published");
    assert_eq!(legacy.fence().as_deref(), Some("4\n"));
    assert!(legacy.exists("db") && !legacy.exists("rollback"));
}
