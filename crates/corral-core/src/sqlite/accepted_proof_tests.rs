use super::*;
use crate::{Artifact, HistorySummary, Run, Status, Store};

#[test]
fn old_database_backfills_current_run_proof_before_hot_readiness() {
    // Given a pre-metadata database containing a purged accepted artifact.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let mut task = store.create("Accepted", None, Status::Done, "").unwrap();
    task.runs.push(
        serde_json::from_value::<Run>(serde_json::json!({
            "id":"R1", "agent":"fixture", "worktree":"/unused", "branch":"branch",
            "base":"main", "base_sha":"0".repeat(40), "started":"2026-01-01T00:00:00Z",
            "outcome":"done"
        }))
        .unwrap(),
    );
    task.artifacts.push(
        serde_json::from_value::<Artifact>(serde_json::json!({
            "run":"R1", "kind":"report", "title":"Proof", "path":"artifacts/proof.md",
            "source":"/tmp/proof.md", "sha256":"0".repeat(64), "bytes":1,
            "added":"2026-01-01T00:00:00Z", "purged_at":"2026-01-02T00:00:00Z"
        }))
        .unwrap(),
    );
    store.fixture_put_task(&task).unwrap();
    let path = crate::sqlite::SqliteBackend::db_path(store.dir());
    drop(store);
    let db = Connection::open(path).unwrap();
    db.execute_batch(
        "DELETE FROM meta WHERE key = 'accepted_proof_metadata';
        UPDATE tasks SET history = json_remove(history, '$.current_artifacts.accepted_proof');",
    )
    .unwrap();
    drop(db);

    // When the real store opens, observe the one-time preparation as a negative control.
    let before = crate::work::snapshot();
    let store = {
        let _whole_call = crate::work::HotScope::enter();
        Store::open(dir.path()).unwrap()
    };
    let preparation = crate::work::snapshot().since(&before);
    assert_eq!(preparation.get(crate::work::Counter::HistoryRowDecodes), 1);
    assert_eq!(preparation.unattributed_hydrations(), 0);
    preparation.check_identity().unwrap();
    drop(store);

    // A second production open includes preparation but must never replay that history.
    let before = crate::work::snapshot();
    let store = {
        let _whole_call = crate::work::HotScope::enter();
        Store::open(dir.path()).unwrap()
    };
    let warm = crate::work::snapshot().since(&before);
    assert_eq!(warm.get(crate::work::Counter::HistoryRowDecodes), 0);
    assert_eq!(warm.unattributed_hydrations(), 0);
    warm.check_identity().unwrap();

    let before = crate::work::snapshot();
    let head = {
        let _hot = crate::work::HotScope::enter();
        store.head(&task.id).unwrap()
    };

    // Then facts match a full recount without decoding any artifact row in the hot path.
    assert_eq!(head.summary, HistorySummary::of(&task).unwrap());
    let proof = &head.summary.current_artifacts.accepted_proof;
    assert_eq!(
        proof.first_purged.as_ref().unwrap().path,
        task.artifacts[0].path
    );
    let work = crate::work::snapshot().since(&before);
    assert_eq!(work.get(crate::work::Counter::HistoryRowDecodes), 0);
    assert_eq!(work.unattributed_hydrations(), 0);
    work.check_identity().unwrap();
}
