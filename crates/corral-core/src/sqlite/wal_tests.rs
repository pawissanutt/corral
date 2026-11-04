use super::*;

#[test]
fn expired_reader_releases_wal_without_waiting_for_its_owner() {
    let root = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(jiff::Timestamp::UNIX_EPOCH));
    let backend = SqliteBackend::open(
        root.path(),
        SqliteOptions {
            clock: clock.clone(),
            ..SqliteOptions::default()
        },
    )
    .unwrap();
    backend.writer().busy_timeout(Duration::ZERO).unwrap();
    let reader = backend.view_with_max_age(Duration::from_secs(5)).unwrap();
    backend
        .apply(Operation::PutRecords(PutRecords {
            writes: vec![RecordWrite {
                area: "wal-test".into(),
                key: "one".into(),
                expected: None,
                doc: Some(serde_json::json!({"value": 1})),
                updated: clock.now(),
            }],
            events: Vec::new(),
            reservations: Vec::new(),
        }))
        .unwrap();
    let before = std::fs::metadata(backend.path.with_extension("db-wal"))
        .unwrap()
        .len();
    assert!(before > 0 && before < WAL_LIMIT_BYTES);
    clock.advance(Duration::from_secs(6));
    let checkpoint = backend.checkpoint_truncate().unwrap();
    assert!(!checkpoint.busy, "expired idle reader pinned the WAL");
    assert_eq!(
        std::fs::metadata(backend.path.with_extension("db-wal"))
            .unwrap()
            .len(),
        0
    );
    assert!(matches!(
        reader.ensure_fresh(),
        Err(ViewError::Expired { .. })
    ));
    assert_eq!(backend.pragmas().unwrap().synchronous, 1);
    println!("WAL before={before} after=0 limit={WAL_LIMIT_BYTES}; expired owner is still alive");
    let fixture = SqliteBackend::open(
        &root.path().join("throwaway"),
        SqliteOptions {
            fixture_no_sync: true,
            ..SqliteOptions::default()
        },
    )
    .unwrap();
    assert_eq!(fixture.pragmas().unwrap().synchronous, 0);
}
