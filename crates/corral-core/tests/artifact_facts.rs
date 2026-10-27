mod store_support;

use std::collections::BTreeSet;
use std::sync::Arc;

use corral_core::backend::fake::FakeBackend;
use corral_core::backend::{BackendConfig, FaultInjector, ManualClock, PageReq};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{Artifact, ArtifactKind, ArtifactTier, CardField, HistorySummary, Status, Store};
use store_support::{at, coding_run, store};

fn artifact(run: &str, kind: ArtifactKind, path: &str) -> Artifact {
    Artifact {
        run: run.into(),
        kind,
        path: path.into(),
        source: "/tmp/report.md".into(),
        title: if kind == ArtifactKind::Diff {
            "Final diff +12 -3"
        } else {
            "Report"
        }
        .into(),
        sha256: "0".repeat(64),
        bytes: 1,
        added: at(0),
        tier_override: None,
        compressed_bytes: None,
        purged_at: None,
    }
}

fn assert_facts(store: &Store, id: &str) {
    let task = store.get(id).unwrap();
    let head = store.head(id).unwrap();
    assert_eq!(head.summary, HistorySummary::of(&task).unwrap());
    let reviewed: BTreeSet<String> = store
        .record("reviewed-artifacts", "set")
        .unwrap()
        .unwrap_or_default();
    assert_eq!(
        head.reviewed_keep_artifacts().unwrap(),
        u64::try_from(
            task.artifacts
                .iter()
                .filter(|a| a.purged_at.is_none()
                    && a.tier() == ArtifactTier::Keep
                    && reviewed.contains(&a.path))
                .count()
        )
        .unwrap()
    );
    for field in [CardField::Artifacts, CardField::Lines] {
        assert_eq!(
            head.artifact_card_text(field),
            field.text(&task, &[], at(0), false)
        );
    }
}

#[test]
fn artifact_facts_follow_appends_full_edits_purges_and_mark_recounts_on_both_backends() {
    for fake in [false, true] {
        // Given both backends, repeated paths and marks predating a registration.
        let (_dir, base) = store();
        let store = if fake {
            base.with_backend(Box::new(FakeBackend::new(
                BackendConfig::default(),
                Arc::new(ManualClock::new(at(0))),
                Arc::new(FaultInjector::default()),
            )))
        } else {
            base
        };
        let mut task = store.create("Facts", None, Status::Review, "").unwrap();
        task.runs.push(coding_run(&task.id, "R1", 0, 0, 0, false));
        store.fixture_put_task(&task).unwrap();
        let path = format!("artifacts/{}/R1/report.md", task.id);
        store
            .save_record("reviewed-artifacts", "set", &BTreeSet::from([path.clone()]))
            .unwrap();
        // When appends, full writes and reviewed toggles change the authoritative data.
        for kind in [
            ArtifactKind::Diff,
            ArtifactKind::Report,
            ArtifactKind::Report,
        ] {
            store
                .update_head(&task.id, |head| {
                    head.add_artifact(artifact("R1", kind, &path))
                })
                .unwrap();
            assert_facts(&store, &task.id);
        }
        assert_eq!(
            store
                .head(&task.id)
                .unwrap()
                .reviewed_keep_artifacts()
                .unwrap(),
            2
        );
        store
            .update(&task.id, |task| {
                task.artifacts[1].purged_at = Some(at(20));
                task.artifacts[2].tier_override = Some(ArtifactTier::Transient);
                task.artifacts[0].title = "Final diff +50 -9".into();
                Ok(())
            })
            .unwrap();
        // Then the stored facts equal an independent full-task recount after the full write.
        assert_facts(&store, &task.id);
        assert_eq!(
            store
                .head(&task.id)
                .unwrap()
                .reviewed_keep_artifacts()
                .unwrap(),
            0
        );
        store
            .update(&task.id, |task| {
                task.artifacts[1].purged_at = None;
                task.artifacts[2].tier_override = None;
                Ok(())
            })
            .unwrap();
        assert_facts(&store, &task.id);
        store
            .save_record("reviewed-artifacts", "set", &BTreeSet::<String>::new())
            .unwrap();
        assert_facts(&store, &task.id);
        store
            .update(&task.id, |task| {
                task.artifacts.remove(1);
                task.artifacts[1].path = "another-path".into();
                Ok(())
            })
            .unwrap();
        store
            .save_record(
                "reviewed-artifacts",
                "set",
                &BTreeSet::from(["another-path".to_owned()]),
            )
            .unwrap();
        assert_facts(&store, &task.id);
        store
            .update_head(&task.id, |head| {
                head.start_run(coding_run(&task.id, "R2", 1, 0, 0, false))
            })
            .unwrap();
        assert_facts(&store, &task.id);
    }
}

#[test]
fn current_run_and_unrelated_reviewed_growth_leave_scalar_and_keyed_reads_constant() {
    let mut reports = Vec::new();
    for factor in [1, 10] {
        // Given tenfold artifacts appended to the active run and reviewed terminal artifacts.
        let (_dir, store) = store();
        let mut task = store.create("Active", None, Status::Review, "").unwrap();
        task.runs.push(coding_run(&task.id, "R1", 0, 0, 0, false));
        task.created = at(0);
        task.updated = at(0);
        store.fixture_put_task(&task).unwrap();
        for i in 0..4 * factor {
            store
                .update_head(&task.id, |head| {
                    head.add_artifact(artifact(
                        "R1",
                        ArtifactKind::Log,
                        &format!("artifacts/{}/R1/{i:04}.log", task.id),
                    ))
                })
                .unwrap();
        }
        let diff = artifact("R1", ArtifactKind::Diff, "artifacts/final.diff");
        store
            .update_head(&task.id, |head| head.add_artifact(diff.clone()))
            .unwrap();
        let mut marks = BTreeSet::new();
        for i in 0..4 * factor {
            let mut old = store.create("Old", None, Status::Dropped, "").unwrap();
            let path = format!("artifacts/{}/R0/{i:04}.md", old.id);
            old.artifacts
                .push(artifact("R0", ArtifactKind::Report, &path));
            marks.insert(path);
            store.fixture_put_task(&old).unwrap();
        }
        store
            .save_record("reviewed-artifacts", "set", &marks)
            .unwrap();
        let mut full = store.get(&task.id).unwrap();
        // Timestamp precision must not make a byte-budget comparison depend on wall time.
        full.updated = at(0);
        store.fixture_put_task(&full).unwrap();
        let before = work::snapshot();
        // When the same head scalars and keyed reads are evaluated in HotScope.
        {
            let _hot = HotScope::enter();
            let head = store.head(&task.id).unwrap();
            for field in [CardField::Artifacts, CardField::Lines] {
                assert_eq!(
                    head.artifact_card_text(field),
                    field.text(&full, &[], at(0), false)
                );
            }
            assert_eq!(
                head.latest_artifact_kind("R1", ArtifactKind::Diff).unwrap(),
                Some(diff)
            );
            assert_eq!(head.reviewed_keep_artifacts().unwrap(), 0);
            assert!(
                head.artifacts_by_title("R1", "PM isolated test result", PageReq::first(1))
                    .unwrap()
                    .items
                    .is_empty()
            );
        }
        // Then every unattributed cell, including decoded bytes, is unchanged.
        let report = work::snapshot().since(&before);
        assert_eq!(report.unattributed_hydrations(), 0);
        report.check_identity().unwrap();
        reports.push(report);
    }
    for counter in Counter::ALL {
        assert_eq!(
            reports[0].get(counter),
            reports[1].get(counter),
            "{counter:?}"
        );
    }
}

#[test]
fn old_sqlite_boards_initialize_artifact_facts_and_indexes_at_open() {
    // Given a database with registrations and reviewed marks but no F3 derived tables.
    let (root, store) = store();
    let mut task = store.create("Old board", None, Status::Review, "").unwrap();
    task.runs.push(coding_run(&task.id, "R1", 0, 0, 0, false));
    task.artifacts
        .push(artifact("R1", ArtifactKind::Report, "shared-path"));
    task.artifacts
        .push(artifact("R1", ArtifactKind::Diff, "final-path"));
    store.fixture_put_task(&task).unwrap();
    store
        .save_record(
            "reviewed-artifacts",
            "set",
            &BTreeSet::from(["shared-path"]),
        )
        .unwrap();
    let path = corral_core::sqlite::SqliteBackend::db_path(store.dir());
    drop(store);
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch(
        "DROP TABLE artifact_reviewed_paths; DROP TABLE artifact_keep_paths;
        DROP TABLE artifact_review_counts;
        UPDATE tasks SET history = json_remove(history, '$.current_artifacts');
        DELETE FROM task_row_index WHERE token LIKE 'run-%';",
    )
    .unwrap();
    drop(db);
    // When production open encounters that older database.
    let store = Store::open(root.path()).unwrap();
    // Then all derived values and new keyed lookups agree with the full task.
    assert_facts(&store, &task.id);
    let head = store.head(&task.id).unwrap();
    assert_eq!(
        head.artifact_in_run("R1", "shared-path").unwrap(),
        Some(task.artifacts[0].clone())
    );
    assert_eq!(
        head.latest_artifact_kind("R1", ArtifactKind::Diff).unwrap(),
        Some(task.artifacts[1].clone())
    );
}
