use super::*;
use corral_core::backend::{BackendConfig, ManualClock, ViewError, fake::FakeBackend};
use std::{sync::Arc, time::Duration};

#[test]
fn obligation_phase_renews_after_external_work_but_not_explicit_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp::UNIX_EPOCH));
    let store = corral_core::Store::init(dir.path())
        .unwrap()
        .with_backend(Box::new(FakeBackend::new(
            BackendConfig::default(),
            clock.clone(),
            Arc::default(),
        )));
    let ctx = Ctx::from_store(store);
    let task = ctx
        .store
        .create("Waiting", None, Status::Running, "")
        .unwrap();
    ctx.store.update(&task.id, |task| {
        let mut run: corral_core::Run = serde_json::from_value(json!({
            "id":"R1", "agent":"worker", "worktree":dir.path(), "branch":"main",
            "base":"main", "base_sha":"base", "started":Timestamp::UNIX_EPOCH,
            "obligations":{"waits":[{
                "request":{"version":1,"id":"W1","run":"R1","condition":"external",
                    "owner":"worker","source":"worker","deadline":null,
                    "handle":{"kind":"channel","channel_id":"fixture","capability":"wait_v1"}},
                "registered_at":Timestamp::UNIX_EPOCH,
                "observation":{"at":Timestamp::UNIX_EPOCH,"state":"pending","detail":"waiting for a result"}
            }]}
        }))?;
        let receipt = serde_json::from_value(json!({
            "version":1,"id":"reply-W1","run":"R1","outcome":"answered",
            "delivery":"acknowledged","observed_at":Timestamp::UNIX_EPOCH
        }))?;
        run.obligations.waits[0].observation =
            crate::obligations::receipt_observation(&receipt, Timestamp::UNIX_EPOCH);
        run.obligations.waits[0].receipt = Some(receipt);
        task.runs.push(run);
        Ok(())
    }).unwrap();
    let head = ctx.store.head(&task.id).unwrap();
    let frozen = Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    };
    // Model the unbounded work between collecting heads and the obligation read phase.
    clock.advance(Duration::from_secs(31));
    let result = crate::obligations_sync::reconcile_head(&ctx, &head, Timestamp::UNIX_EPOCH, None);
    assert!(result.unwrap().is_none());
    assert_eq!(ctx.store.head(&task.id).unwrap().rev, head.rev);
    let error =
        crate::obligations_sync::reconcile_head(&frozen, &head, Timestamp::UNIX_EPOCH, None)
            .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ViewError>(),
        Some(ViewError::Expired { .. })
    ));
}
