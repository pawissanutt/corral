use super::*;
use corral_core::Store;
use corral_core::backend::{BackendConfig, ManualClock, fake::FakeBackend};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn prompt_preparation_rereads_after_external_work_expires_the_saved_head() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp::UNIX_EPOCH));
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
        .create("Review", None, Status::Review, "")
        .unwrap();
    ctx.store
        .update(&task.id, |task| {
            task.runs.push(serde_json::from_value(json!({
                "id":"R1", "agent":"worker", "worktree":dir.path(), "branch":"main",
                "base":"main", "base_sha":"base", "started":Timestamp::UNIX_EPOCH,
                "outcome":"done"
            }))?);
            Ok(())
        })
        .unwrap();
    let saved = ctx.store.head(&task.id).unwrap();
    let run = current_run_head(&saved).unwrap();

    // Watch/packet/Git work may take longer than the saved head's lease.
    clock.advance(Duration::from_secs(31));
    assert!(saved.emitted_comments().is_err());
    prepare_prompt(&ctx, &saved, &run).unwrap();
    // Renew only the read phase, never revive the old snapshot.
    assert!(saved.emitted_comments().is_err());
}
