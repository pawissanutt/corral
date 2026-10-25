use super::*;

#[test]
fn usage_events_select_only_the_task_and_reviewer_lifecycle_kinds() {
    // Given interleaved tasks and unrelated events on the selected task.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    for (task, kind) in [
        ("T-001", "review_started"),
        ("T-002", "approved"),
        ("T-001", "artifact"),
        ("T-001", "approved"),
    ] {
        store
            .log_event(Some(task), kind, serde_json::json!({"run":"R1"}))
            .unwrap();
    }
    let before = EVENT_READS.get();
    // When usage requests the task's indexed review history.
    let events = usage_events(&store, "T-001").unwrap();
    // Then one query returns only that task's lifecycle events in append order.
    assert_eq!(EVENT_READS.get() - before, 1);
    assert_eq!(
        events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>(),
        ["review_started", "approved"]
    );
    assert!(
        events
            .iter()
            .all(|event| event.task.as_deref() == Some("T-001"))
    );
}

#[test]
fn imported_usage_events_match_database_parsing_including_eof_input() {
    // Given imported events, malformed JSON and a valid EOF input line.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let first = r#"{"task":"T-001","kind":"approved","ts":"2026-10-01T00:00:00Z","detail":{}}"#;
    let second = first.replace("approved", "discarded");
    store
        .fixture_replace_events(&format!("{first}\nmalformed\n{second}"))
        .unwrap();
    let before = EVENT_READS.get();
    // When changed events are refreshed.
    let events = usage_events(&store, "T-001").unwrap();
    // Then one read preserves imported EOF events and skips the stored malformed row.
    assert_eq!(EVENT_READS.get() - before, 1);
    assert_eq!(events.len(), 2);
    assert_eq!(
        serde_json::to_value(&events).unwrap(),
        serde_json::to_value(store.events().unwrap()).unwrap()
    );
}
