use super::*;

#[test]
fn feed_moves_without_task_changes_do_not_resave_the_settlement_cursor() {
    // Given a settled derivation with a persisted cursor.
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(tmp.path()).unwrap());
    assert!(terminal_heads(&ctx).unwrap().is_empty());
    let saved = ctx.store.record_row(AREA, "state").unwrap();
    assert!(saved.is_some());
    // When only a non-task record changes before the next derivation.
    ctx.store
        .save_record("fixture", "state", &serde_json::json!({"n": 1}))
        .unwrap();
    let seq = ctx.store.view().unwrap().seq();
    assert!(terminal_heads(&ctx).unwrap().is_empty());
    // Then the derivation writes nothing.
    assert_eq!(ctx.store.view().unwrap().seq(), seq);
    assert_eq!(ctx.store.record_row(AREA, "state").unwrap(), saved);
}
