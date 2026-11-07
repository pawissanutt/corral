#[path = "../store_support/mod.rs"]
mod store_support;

use std::collections::BTreeSet;

use corral_core::{Artifact, ArtifactKind, CardField, HistorySummary, Status, Store};
use store_support::{at, coding_run, store};

#[test]
fn old_sqlite_boards_initialize_artifact_facts_and_indexes_at_open() {
    // Given a database with registrations and reviewed marks but no F3 derived tables.
    let (root, store) = store();
    let mut task = store.create("Old board", None, Status::Review, "").unwrap();
    task.runs.push(coding_run(&task.id, "R1", 0, 0, 0, false));
    for (kind, path) in [("report", "shared-path"), ("diff", "final-path")] {
        task.artifacts.push(
            serde_json::from_value::<Artifact>(serde_json::json!({
                "run": "R1", "kind": kind, "path": path, "source": "/tmp/report.md",
                "title": if kind == "diff" { "Final diff +12 -3" } else { "Report" },
                "sha256": "0".repeat(64), "bytes": 1, "added": at(0)
            }))
            .unwrap(),
        );
    }
    store.fixture_put_task(&task).unwrap();
    corral_core::records::RecordArea::new("reviewed-artifacts")
        .save(&store, "set", &BTreeSet::from(["shared-path"]))
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
    let head = store.head(&task.id).unwrap();
    assert_eq!(
        head.summary,
        HistorySummary::of_record(&store.task_record(&task.id).unwrap()).unwrap()
    );
    assert_eq!(head.reviewed_keep_artifacts().unwrap(), 1);
    for field in [CardField::Artifacts, CardField::Lines] {
        assert_eq!(
            head.artifact_card_text(field),
            field.text(&task, &[], at(0), false)
        );
    }
    assert_eq!(
        head.artifact_in_run("R1", "shared-path").unwrap(),
        Some(task.artifacts[0].clone())
    );
    assert_eq!(
        head.latest_artifact_kind("R1", ArtifactKind::Diff).unwrap(),
        Some(task.artifacts[1].clone())
    );
}
