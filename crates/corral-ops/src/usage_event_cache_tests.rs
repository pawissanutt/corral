use super::*;

#[test]
fn unchanged_usage_events_reuse_the_snapshot_without_reading() {
    // Given an event snapshot already read from this store.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let first = usage_events(&store).unwrap();
    let before = EVENT_READS.get();
    // When the same store refreshes with unchanged event metadata.
    let again = usage_events(&store).unwrap();
    // Then neither bytes nor the parsed snapshot are copied.
    assert_eq!(EVENT_READS.get(), before);
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
    fs::write(
        store.dir().join("events.jsonl"),
        format!("{first}\nmalformed\n{second}"),
    )
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

#[test]
#[cfg(unix)]
fn replaced_usage_events_invalidate_even_with_restored_mtime() {
    // Given an event replaced at the same length, preserving its original mtime.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let path = store.dir().join("events.jsonl");
    let first = r#"{"task":"T-001","kind":"approved","ts":"2026-10-01T00:00:00Z","detail":{}}"#;
    fs::write(&path, first).unwrap();
    usage_events(&store).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let replacement = tmp.path().join("replacement");
    fs::write(&replacement, first.replace("approved", "discard!")).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    fs::rename(replacement, &path).unwrap();
    // When the event log is refreshed again.
    let events = usage_events(&store).unwrap();
    // Then file identity exposes the replacement despite unchanged size/mtime.
    assert_eq!(events[0].kind, "discard!");
}
