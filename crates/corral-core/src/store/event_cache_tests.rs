use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::{Event, Store};

fn line(kind: &str) -> String {
    let event = Event {
        ts: jiff::Timestamp::from_second(0).unwrap(),
        task: None,
        kind: kind.into(),
        actor: None,
        via: None,
        detail: serde_json::json!({}),
    };
    format!("{}\n", serde_json::to_string(&event).unwrap())
}

fn workspace(text: &str) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    fs::write(store.dir().join("events.jsonl"), text).unwrap();
    (dir, store)
}

fn append(path: &Path, text: &str) {
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

#[test]
fn own_appends_reuse_known_values_only_after_reading_exact_bytes() {
    // Given a warmed log and an event encoded by this store.
    let (_dir, store) = workspace("");
    store.events_shared().unwrap();
    let decodes = store.event_cache.decodes.load(Ordering::Relaxed);
    store
        .log_event(None, "written", serde_json::json!({"value": 1}))
        .unwrap();
    // When a reader consumes the actual appended bytes.
    let events = store.events_shared().unwrap();
    // Then no second JSON decode is needed for bytes whose value is already known.
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "written");
    assert_eq!(store.event_cache.decodes.load(Ordering::Relaxed), decodes);
}

#[test]
fn external_changes_do_not_reuse_an_own_append_value() {
    // Given an own append that has not yet been read.
    let (_dir, store) = workspace("");
    store
        .log_event(None, "written", serde_json::json!({}))
        .unwrap();
    let decodes = store.event_cache.decodes.load(Ordering::Relaxed);
    // When an external writer replaces those bytes.
    fs::write(store.dir().join("events.jsonl"), line("outside")).unwrap();
    // Then the external event is decoded rather than replaced by a remembered value.
    let events = store.events_shared().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "outside");
    assert_eq!(
        store.event_cache.decodes.load(Ordering::Relaxed),
        decodes + 1
    );
}

#[test]
fn partial_own_append_cannot_materialize_an_unread_event() {
    // Given a known event, but only its first half remaining in the actual log.
    let (_dir, store) = workspace("");
    store
        .log_event(None, "written", serde_json::json!({}))
        .unwrap();
    let path = store.dir().join("events.jsonl");
    let bytes = fs::read_to_string(&path).unwrap();
    let split = bytes.len() / 2;
    fs::write(&path, &bytes[..split]).unwrap();
    assert!(store.events_shared().unwrap().is_empty());
    // When the rest of the event is restored.
    append(&path, &bytes[split..]);
    // Then exactly the one complete event becomes visible.
    let events = store.events_shared().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "written");
}

#[test]
fn failed_append_does_not_create_a_phantom_event() {
    // Given a path on which append cannot open a regular log.
    let (_dir, store) = workspace("");
    let path = store.dir().join("events.jsonl");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    // When the append fails.
    assert!(
        store
            .log_event(None, "failed", serde_json::json!({}))
            .is_err()
    );
    fs::remove_dir(&path).unwrap();
    // Then a subsequent reader sees no event.
    assert!(store.events_shared().unwrap().is_empty());
}

#[test]
fn written_event_reuse_is_bounded_and_evicted_lines_still_decode() {
    // Given more own appends than the bounded reuse window.
    let (_dir, store) = workspace("");
    for index in 0..=super::WRITTEN_LINES {
        store
            .log_event(None, "written", serde_json::json!({"index": index}))
            .unwrap();
    }
    let decodes = store.event_cache.decodes.load(Ordering::Relaxed);
    assert_eq!(
        store.event_cache.written.lock().unwrap().len(),
        super::WRITTEN_LINES
    );
    // When all actual bytes are consumed.
    let events = store.events_shared().unwrap();
    // Then the evicted line is decoded normally, with no event loss or duplication.
    assert_eq!(events.len(), super::WRITTEN_LINES + 1);
    assert_eq!(events[0].detail["index"], 0);
    assert_eq!(
        store.event_cache.decodes.load(Ordering::Relaxed),
        decodes + 1
    );
}

#[test]
fn oversized_own_events_are_not_retained_but_remain_readable() {
    // Given an append larger than the bounded remembered-line size.
    let (_dir, store) = workspace("");
    let remembered = store.event_cache.written.lock().unwrap().len();
    store
        .log_event(
            None,
            "large",
            serde_json::json!({"bytes": "x".repeat(super::WRITTEN_LINE_BYTES)}),
        )
        .unwrap();
    let decodes = store.event_cache.decodes.load(Ordering::Relaxed);
    // When the reader consumes that event.
    let events = store.events_shared().unwrap();
    // Then the ordinary decoder serves it without growing the reuse cache.
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].detail["bytes"].as_str().unwrap().len(),
        super::WRITTEN_LINE_BYTES
    );
    assert_eq!(store.event_cache.written.lock().unwrap().len(), remembered);
    assert_eq!(
        store.event_cache.decodes.load(Ordering::Relaxed),
        decodes + 1
    );
}

#[test]
fn unchanged_reads_reuse_decodes_across_store_clones() {
    // Given a warmed snapshot shared with a worker.
    let (_dir, store) = workspace(&line("first"));
    let snapshot = store.events_shared().unwrap();
    let decodes = store.event_cache.decodes.load(Ordering::Relaxed);
    let worker = store.clone();

    // When another thread reads the unchanged log.
    let second = std::thread::spawn(move || worker.events_shared().unwrap())
        .join()
        .unwrap();

    // Then the snapshot and its decoded events are reused.
    assert!(Arc::ptr_eq(&snapshot, &second));
    assert_eq!(store.event_cache.decodes.load(Ordering::Relaxed), decodes);
    assert_eq!(store.events().unwrap(), *snapshot);
}

#[test]
fn complete_appends_decode_only_new_lines() {
    // Given a cached prefix and a reader retaining its original snapshot.
    let (_dir, store) = workspace(&line("first"));
    let snapshot = store.events_shared().unwrap();
    let decodes = store.event_cache.decodes.load(Ordering::Relaxed);
    append(&store.dir().join("events.jsonl"), &line("second"));

    // When the appended complete line is read.
    let events = store.events_shared().unwrap();

    // Then the suffix is visible in order without decoding the prefix again.
    assert_eq!(
        events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(snapshot.len(), 1);
    assert_eq!(
        store.event_cache.decodes.load(Ordering::Relaxed),
        decodes + 1
    );
}

#[test]
fn partial_trailing_line_is_reread_when_completed() {
    // Given an incomplete JSON line after a cached complete event.
    let second = line("second");
    let split = second.len() / 2;
    let (_dir, store) = workspace(&format!("{}{}", line("first"), &second[..split]));
    assert_eq!(store.events_shared().unwrap().len(), 1);
    append(&store.dir().join("events.jsonl"), &second[split..]);

    // When the writer completes that line.
    let events = store.events_shared().unwrap();

    // Then the complete event appears exactly once.
    assert_eq!(
        events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        ["first", "second"]
    );
}

#[test]
fn valid_unterminated_line_is_visible_without_committing_it() {
    // Given valid JSON without its final newline.
    let text = line("first");
    let (_dir, store) = workspace(text.trim_end_matches('\n'));
    let snapshot = store.events_shared().unwrap();
    assert_eq!(snapshot[0].kind, "first");
    append(
        &store.dir().join("events.jsonl"),
        &format!(" \r\n{}", line("second")),
    );

    // When the writer terminates it and adds another event.
    let events = store.events_shared().unwrap();

    // Then the provisional event is replaced, not duplicated or lost.
    assert_eq!(
        events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        ["first", "second"]
    );
}

#[test]
fn shrinking_log_replaces_the_cached_prefix() {
    // Given two cached events.
    let (_dir, store) = workspace(&format!("{}{}", line("first"), line("second")));
    store.events_shared().unwrap();
    fs::write(store.dir().join("events.jsonl"), line("third")).unwrap();

    // When the same file has been truncated to one event.
    let events = store.events_shared().unwrap();

    // Then only the current file's contents remain.
    assert_eq!(
        *events,
        vec![serde_json::from_str::<Event>(&line("third")).unwrap()]
    );
}

#[test]
fn renamed_replacement_invalidates_even_with_a_matching_anchor() {
    // Given a cached log whose replacement has the same length and trailing bytes.
    let suffix = format!("{}{}", " ".repeat(128), "\n");
    let original = format!("{}{suffix}", line("first"));
    let replacement = format!("{}{suffix}", line("other"));
    assert_eq!(original.len(), replacement.len());
    let (_dir, store) = workspace(&original);
    store.events_shared().unwrap();
    let path = store.dir().join("events.jsonl");
    let temp = path.with_extension("replacement");
    fs::write(&temp, replacement).unwrap();
    fs::rename(temp, path).unwrap();

    // When a new inode occupies the path.
    let events = store.events_shared().unwrap();

    // Then identity, rather than length or anchor, invalidates the snapshot.
    assert_eq!(events[0].kind, "other");
}

#[test]
fn rewritten_or_regrown_log_invalidates_the_anchor() {
    // Given a cached complete line, with the same inode retained by each rewrite.
    for suffix in ["", "\nextra malformed text\n"] {
        let (_dir, store) = workspace(&line("first"));
        store.events_shared().unwrap();
        fs::write(
            store.dir().join("events.jsonl"),
            format!("{}{suffix}", line("other")),
        )
        .unwrap();

        // When an in-place rewrite preserves or increases length.
        let events = store.events_shared().unwrap();

        // Then the anchor detects changes that an offset alone cannot.
        assert_eq!(events[0].kind, "other");
    }
}

#[test]
fn malformed_lines_are_skipped_with_str_lines_semantics() {
    // Given CRLF, blank and malformed lines between valid events.
    let text = format!(
        "{}\r\nbad JSON\n\n{}",
        line("first").trim_end_matches('\n'),
        line("second")
    );
    let (_dir, store) = workspace(&text);

    // When the log is read.
    let events = store.events_shared().unwrap();

    // Then only valid events remain, newest last.
    assert_eq!(
        events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        ["first", "second"]
    );
}

#[test]
fn missing_log_clears_a_previously_cached_snapshot() {
    // Given a warmed log that has since been removed.
    let (_dir, store) = workspace(&line("first"));
    store.events_shared().unwrap();
    fs::remove_file(store.dir().join("events.jsonl")).unwrap();

    // When the missing path is read.
    let events = store.events_shared().unwrap();

    // Then both the initial missing read and subsequent reads are empty.
    assert!(events.is_empty());
    assert!(store.events().unwrap().is_empty());
}
