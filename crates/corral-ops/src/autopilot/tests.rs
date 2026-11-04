use super::*;
use corral_core::{Status, Store};

#[test]
fn step_diagnostics_survive_expiry_after_external_work() {
    use corral_core::backend::{BackendConfig, ManualClock, fake::FakeBackend};
    use std::sync::Arc;
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp::UNIX_EPOCH));
    let store = Store::init(dir.path())
        .unwrap()
        .with_backend(Box::new(FakeBackend::new(
            BackendConfig::default(),
            clock.clone(),
            Arc::default(),
        )));
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let task = ctx
        .store
        .create("Review", None, Status::Review, "")
        .unwrap();
    ctx.store
        .update(&task.id, |task| {
            task.runs.push(serde_json::from_value(json!({
                "id":"R1", "agent":"worker", "worktree":dir.path(),
                "branch":"main", "base":"main", "base_sha":"base",
                "started":Timestamp::UNIX_EPOCH, "outcome":"done",
                "review_runs":[{
                    "id":"RR1", "agent":"reviewer", "worktree":dir.path(),
                    "branch":"main", "base":"main", "base_sha":"base",
                    "started":Timestamp::UNIX_EPOCH
                }]
            }))?);
            Ok(())
        })
        .unwrap();
    let head = ctx.store.head(&task.id).unwrap();
    let open_reviews = head.open_review_children("R1").unwrap();
    let head = head.detach();
    let run = head.current_run.as_deref().unwrap();
    let pilot = Pilot {
        ctx: &ctx,
        config: &ctx.store.config,
        head: &head,
        open_reviews,
        run,
        policy: ctx.store.config.task_review_policy(&head),
        now: Timestamp::UNIX_EPOCH,
    };

    // Model a slow external step, then the committed cleanup it completed. No wall-clock wait.
    clock.advance(Duration::from_secs(31));
    ctx.store
        .update(&task.id, |task| {
            task.runs[0].review_runs[0].ended = Some(Timestamp::UNIX_EPOCH);
            Ok(())
        })
        .unwrap();
    pilot
        .log(
            AutoStage::Escalated,
            0,
            None,
            Some("fixture"),
            "handed to owner",
        )
        .unwrap();
    let event = ctx
        .store
        .events_of(Some(&task.id), &["autopilot"])
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(event.detail["released_review_runs"], json!(["RR1"]));
}

#[test]
fn skipped_lease_preserves_error_until_evaluated_pass() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(dir.path()).unwrap());
    let task = ctx
        .store
        .create("Review", None, Status::Review, "")
        .unwrap();
    ctx.store
        .observe_condition(
            Some(&task.id),
            "autopilot_error",
            json!({"error": "failure"}),
        )
        .unwrap();
    let lock = ctx
        .store
        .try_lock(&format!("autopilot-{}", task.id))
        .unwrap()
        .unwrap();
    advance_all(&ctx, corral_core::clock::now()).unwrap();
    let runs = ctx.store.condition_runs(Some(&task.id)).unwrap();
    assert_eq!(runs.len(), 1);
    assert!(runs[0].closed_seq.is_none());
    drop(lock);
    advance_all(&ctx, corral_core::clock::now()).unwrap();
    let runs = ctx.store.condition_runs(Some(&task.id)).unwrap();
    assert!(runs[0].closed_seq.is_some());
}
