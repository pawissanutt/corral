use super::*;
use crate::{Artifact, ArtifactKind, Status, Store};
use serde_json::json;

#[test]
fn existing_database_backfills_lesson_projections_before_opening_views() {
    // Given previously published artifacts and approval events without the new indexes.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let mut task = store
        .create("Historical", Some("app"), Status::Done, "")
        .unwrap();
    let at: jiff::Timestamp = "2026-10-01T00:00:00Z".parse().unwrap();
    for i in 0..40 {
        task.artifacts.push(Artifact {
            run: "R1".into(),
            kind: if i % 3 == 0 {
                ArtifactKind::Plan
            } else {
                ArtifactKind::Report
            },
            title: "Report".into(),
            path: format!("report-{i:04}"),
            source: "/unused".into(),
            sha256: String::new(),
            bytes: 0,
            added: at,
            tier_override: None,
            compressed_bytes: None,
            purged_at: None,
        });
    }
    store.fixture_put_task(&task).unwrap();
    store
        .log_event(Some(&task.id), "approved", json!({"sha":"sha","run":"R1"}))
        .unwrap();
    let db_path = store.dir().join("db/corral.db");
    let num = store.head(&task.id).unwrap().num;
    drop(store);
    let db = Connection::open(&db_path).unwrap();
    db.execute_batch("DROP TABLE lesson_artifacts; DROP TABLE lesson_approvals;")
        .unwrap();
    drop(db);

    // When the production store opener initializes an existing database.
    let reopened = Store::discover(dir.path()).unwrap();
    let frozen = reopened.read_only_snapshot().unwrap();
    let view = frozen.view().unwrap();

    // Then both projections equal the original publications, on a single pinned view.
    let mut expected = task
        .artifacts
        .into_iter()
        .filter(|a| a.kind == ArtifactKind::Report)
        .collect::<Vec<_>>();
    expected.sort_by(|a, b| (b.added, &b.path).cmp(&(a.added, &a.path)));
    expected.truncate(32);
    let selected = view
        .lesson_artifacts(num, 32)
        .unwrap()
        .into_iter()
        .map(|row| serde_json::from_value::<Artifact>(row.doc).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(selected, expected);
    assert_eq!(
        view.lesson_approvals("sha").unwrap(),
        vec![crate::backend::LessonApproval {
            task: task.id.clone(),
            run: Some("R1".into()),
        }]
    );
    reopened
        .log_event(Some(&task.id), "approved", json!({"sha":"sha","run":"R2"}))
        .unwrap();
    assert_eq!(view.lesson_approvals("sha").unwrap().len(), 1);
    assert_eq!(
        reopened
            .view()
            .unwrap()
            .lesson_approvals("sha")
            .unwrap()
            .len(),
        2
    );
}
