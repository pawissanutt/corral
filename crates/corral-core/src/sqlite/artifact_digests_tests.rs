use super::*;
use crate::work::{self, Counter, HotScope};
use crate::{Artifact, ArtifactKind, Status, Store};

#[test]
fn old_rows_backfill_first_digest_order_and_keep_exact_token_deletion_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let mut task = store
        .create("Legacy artifact rows", None, Status::Review, "")
        .unwrap();
    let at = "2026-10-18T00:00:00Z".parse().unwrap();
    let make = |run: &str, file: &str, purged: bool| Artifact {
        run: run.into(),
        kind: ArtifactKind::Log,
        title: file.into(),
        path: format!("artifacts/T-001/{run}/{file}"),
        source: file.into(),
        sha256: "same-content".into(),
        bytes: 4,
        added: at,
        tier_override: None,
        compressed_bytes: purged.then_some(0),
        purged_at: purged.then_some(at),
    };
    task.artifacts = vec![
        make("R0", "first-other-run", false),
        make("R1", "first-purged", true),
        make("R1", "later-live", false),
    ];
    store.fixture_put_task(&task).unwrap();
    let canonical = store.get(&task.id).unwrap();
    let db_path = super::super::SqliteBackend::db_path(store.dir());
    drop(store);
    // This is an independently constructed pre-index database, not a cache-loss simulation:
    // remove both the physical selector and its row-token metadata and initialization marker.
    let db = Connection::open(&db_path).unwrap();
    db.execute("DELETE FROM meta WHERE key = 'artifact_digest_index'", [])
        .unwrap();
    db.execute(
        "DELETE FROM task_row_index WHERE token LIKE 'run-sha:%'",
        [],
    )
    .unwrap();
    for (key, idx) in load::all(
        &db,
        "SELECT key, idx FROM task_rows WHERE part='artifact'",
        [],
        |r| Ok((load::text(r, 0)?, load::text(r, 1)?)),
    )
    .unwrap()
    {
        let mut tokens: Vec<String> = serde_json::from_str(&idx).unwrap();
        tokens.retain(|s| !s.starts_with("run-sha:"));
        db.execute(
            "UPDATE task_rows SET idx=?2 WHERE part='artifact' AND key=?1",
            params![key, serde_json::to_string(&tokens).unwrap()],
        )
        .unwrap();
    }
    drop(db);
    // Initialization derives tokens from authoritative row identity, retaining purged/duplicate rows.
    let reopened = Store::open(dir.path()).unwrap();
    assert_eq!(reopened.get(&task.id).unwrap(), canonical);
    let head = reopened.head(&task.id).unwrap();
    let hot = HotScope::enter();
    let before = work::snapshot();
    assert_eq!(
        head.artifact_registration_by_sha("R1", "same-content")
            .unwrap(),
        Some((1, canonical.artifacts[1].clone()))
    );
    let delta = work::snapshot().since(&before);
    drop(hot);
    assert_eq!(delta.get(Counter::HistoryRowDecodes), 1);
    assert_eq!(delta.unattributed_hydrations(), 0);
    let db = Connection::open(&db_path).unwrap();
    let actual: Vec<(String, String)> = load::all(
        &db,
        "SELECT token,key FROM task_row_index WHERE token LIKE 'run-sha:%' ORDER BY key",
        [],
        |r| Ok((load::text(r, 0)?, load::text(r, 1)?)),
    )
    .unwrap();
    assert_eq!(
        actual,
        vec![
            ("run-sha:2:R0:same-content".into(), "0000000001".into()),
            ("run-sha:2:R1:same-content".into(), "0000000002".into()),
            ("run-sha:2:R1:same-content".into(), "0000000003".into())
        ]
    );
    drop(db);
    reopened
        .update(&task.id, |t| {
            t.artifacts[1].sha256 = "changed".into();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        reopened
            .head(&task.id)
            .unwrap()
            .artifact_registration_by_sha("R1", "same-content")
            .unwrap(),
        Some((2, canonical.artifacts[2].clone()))
    );
}
