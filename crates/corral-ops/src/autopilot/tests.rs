use super::*;
use corral_core::{Status, Store};

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
