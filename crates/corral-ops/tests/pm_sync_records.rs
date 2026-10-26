use corral_core::Store;
use corral_ops::{Ctx, agent::hooks::sync};

#[test]
fn pm_sync_skips_an_absent_record() {
    // Given a store without a Pi-native PM record.
    let temp = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(temp.path()).unwrap());

    // When the PM wake pass runs.
    let result = sync::pass(&ctx);

    // Then it has no PM work and succeeds.
    result.unwrap();
}

#[test]
fn pm_sync_reads_the_canonical_record_without_a_legacy_file() {
    // Given a malformed canonical PM record and no legacy PM state file.
    let temp = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(temp.path()).unwrap());
    ctx.store
        .save_record("agent-pm", "current", &serde_json::json!({}))
        .unwrap();

    // When the PM wake pass runs.
    let result = sync::pass(&ctx);

    // Then decoding the persisted record fails instead of silently skipping the pass.
    assert!(result.is_err());
}
