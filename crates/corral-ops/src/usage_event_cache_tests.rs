use super::*;

#[test]
fn unchanged_usage_events_request_the_shared_snapshot_once() {
    // Given an event snapshot already read from this store.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let first = usage_events(&store).unwrap();
    let before = EVENT_READS.get();
    // When the same store refreshes with unchanged event metadata.
    let again = usage_events(&store).unwrap();
    // Then one shared-events call reuses the parsed snapshot.
    assert_eq!(EVENT_READS.get() - before, 1);
    assert!(std::sync::Arc::ptr_eq(&first, &again));
}

#[test]
fn appended_usage_events_match_store_parsing_including_partial_tail() {
    // Given a cached empty log, followed by events, malformed JSON and a valid EOF tail.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    usage_events(&store).unwrap();
    let first = r#"{"task":"T-001","kind":"approved","ts":"2026-10-01T00:00:00Z","detail":{}}"#;
    let second = first.replace("approved", "discarded");
    store
        .fixture_replace_events(&format!("{first}\nmalformed\n{second}"))
        .unwrap();
    let before = EVENT_READS.get();
    // When changed events are refreshed.
    let events = usage_events(&store).unwrap();
    // Then one read preserves Store's full parser semantics, including the EOF event.
    assert_eq!(EVENT_READS.get() - before, 1);
    assert_eq!(events.len(), 2);
    assert_eq!(
        serde_json::to_value(&*events).unwrap(),
        serde_json::to_value(store.events().unwrap()).unwrap()
    );
}
