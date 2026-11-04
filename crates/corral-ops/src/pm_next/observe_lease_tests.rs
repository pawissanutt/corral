use super::*;
use corral_core::backend::{BackendConfig, ManualClock, ViewError, fake::FakeBackend};
use std::time::Duration;

#[test]
fn backfill_pages_do_not_hold_a_lease_across_artifact_work() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(jiff::Timestamp::UNIX_EPOCH));
    let store = Store::init(dir.path())
        .unwrap()
        .with_backend(Box::new(FakeBackend::new(
            BackendConfig::default(),
            clock.clone(),
            Arc::default(),
        )));
    let ctx = Ctx::from_store(store);
    let task = ctx
        .store
        .create("Legacy", None, corral_core::Status::Review, "")
        .unwrap();
    ctx.store
        .update(&task.id, |task| {
            for n in 0..2 {
                task.artifacts
                    .push(serde_json::from_value(serde_json::json!({
                        "run":"R1", "kind":"report", "title":TITLE, "source":"report.json",
                        "path":format!("report-{n}.json"), "sha256":"digest", "bytes":1,
                        "added":jiff::Timestamp::UNIX_EPOCH
                    }))?);
            }
            Ok(())
        })
        .unwrap();
    let head = ctx.store.head(&task.id).unwrap();
    let frozen = Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    };
    let mut consumed = Vec::new();
    scan_reports(&ctx, &head, "R1", 1, |row| {
        consumed.push(row.key);
        // Artifact verification is external work, performed exactly once per row.
        clock.advance(Duration::from_secs(31));
        Ok(())
    })
    .unwrap();
    assert_eq!(consumed.len(), 2);
    assert_ne!(consumed[0], consumed[1]);
    let error = scan_reports(&frozen, &head, "R1", 1, |_| Ok(())).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ViewError>(),
        Some(ViewError::Expired { .. })
    ));
}
