use super::*;
use std::io::Write;

fn line(task: &str, kind: &str) -> String {
    format!("{{\"ts\":\"2026-10-15T00:00:00Z\",\"task\":\"{task}\",\"kind\":\"{kind}\"}}\n")
}

fn append(path: &Path, text: &str) {
    fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

#[test]
fn starts_at_eof_and_emits_only_new_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events");
    fs::write(&path, line("T-001", "done")).unwrap();
    let mut reader = EventReader::open(&path, None).unwrap();
    assert!(reader.next_event().unwrap().is_none());
    append(&path, &line("T-002", "done"));
    assert_eq!(
        reader.next_event().unwrap().unwrap().0.task.as_deref(),
        Some("T-002")
    );
}

#[test]
fn cursor_resumes_gaplessly_without_read_ahead() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events");
    let first = line("T-001", "done");
    fs::write(&path, format!("{first}{}", line("T-002", "stuck"))).unwrap();
    let mut reader = EventReader::from_offset(&path, 0).unwrap();
    let (_, cursor) = reader.next_event().unwrap().unwrap();
    assert_eq!(cursor.offset, u64::try_from(first.len()).unwrap());
    assert_eq!(reader.cursor(), &cursor);
    let persisted = serde_json::to_string(&cursor).unwrap();
    let mut resumed =
        EventReader::open(&path, Some(serde_json::from_str(&persisted).unwrap())).unwrap();
    assert_eq!(resumed.next_event().unwrap().unwrap().0.kind, "stuck");
    let mut offset = EventReader::from_offset(&path, cursor.offset).unwrap();
    assert_eq!(offset.next_event().unwrap().unwrap().0.kind, "stuck");
}

#[test]
fn retains_partial_lines_and_skips_malformed_and_oversized_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events");
    fs::write(&path, "bad\n").unwrap();
    let mut reader = EventReader::from_offset(&path, 0).unwrap();
    let text = line("T-003", "needs_me");
    append(&path, &text[..20]);
    assert!(reader.next_event().unwrap().is_none());
    assert_eq!(reader.cursor().offset, 4);
    append(&path, &text[20..]);
    assert_eq!(reader.next_event().unwrap().unwrap().0.kind, "needs_me");
    append(&path, &"x".repeat(EVENT_READ_LIMIT + 100));
    assert!(reader.next_event().unwrap().is_none());
    assert!(reader.partial.len() <= EVENT_READ_LIMIT);
    append(&path, &format!("\n{}", line("T-004", "done")));
    assert_eq!(reader.next_event().unwrap().unwrap().0.kind, "done");
}

#[test]
fn eof_in_partial_line_discards_old_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events");
    let text = line("T-001", "done");
    fs::write(&path, &text[..20]).unwrap();
    let mut reader = EventReader::open(&path, None).unwrap();
    append(&path, &format!("{}{}", &text[20..], line("T-002", "done")));
    assert_eq!(
        reader.next_event().unwrap().unwrap().0.task.as_deref(),
        Some("T-002")
    );
}

#[test]
fn follows_truncation_regrowth_and_replacement_even_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events");
    fs::write(&path, line("T-001", "done")).unwrap();
    let mut reader = EventReader::open(&path, None).unwrap();
    fs::write(&path, "").unwrap();
    assert!(reader.next_event().unwrap().is_none());
    append(&path, &line("T-002", "stuck"));
    let (_, cursor) = reader.next_event().unwrap().unwrap();
    fs::write(&path, line("T-003", "needs_me")).unwrap();
    let mut resumed = EventReader::open(&path, Some(cursor)).unwrap();
    assert_eq!(resumed.next_event().unwrap().unwrap().0.kind, "needs_me");
    let cursor = resumed.cursor().clone();
    fs::rename(&path, dir.path().join("old")).unwrap();
    fs::write(&path, line("T-004", "blocked")).unwrap();
    let mut resumed = EventReader::open(&path, Some(cursor)).unwrap();
    assert_eq!(resumed.next_event().unwrap().unwrap().0.kind, "blocked");
}

#[test]
fn missing_log_can_appear_later() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events");
    let mut reader = EventReader::open(&path, None).unwrap();
    assert!(reader.next_event().unwrap().is_none());
    append(&path, &line("T-001", "done"));
    assert!(reader.next_event().unwrap().is_some());
}

#[test]
fn bounded_progress_checkpoints_completed_garbage_but_not_partial_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events");
    fs::write(
        &path,
        format!("{}\npartial", "x".repeat(EVENT_READ_LIMIT + 100)),
    )
    .unwrap();
    let mut reader = EventReader::from_offset(&path, 0).unwrap();
    assert!(reader.next_event().unwrap().is_none());
    assert_eq!(
        reader.read_position(),
        u64::try_from(EVENT_READ_LIMIT).unwrap()
    );
    assert_eq!(reader.cursor().offset, 0);
    assert!(reader.next_event().unwrap().is_none());
    let checkpoint = reader.cursor().clone();
    assert_eq!(
        checkpoint.offset,
        u64::try_from(EVENT_READ_LIMIT + 101).unwrap()
    );
    let end = reader.read_position();
    assert!(reader.next_event().unwrap().is_none());
    assert_eq!(reader.read_position(), end);
    let mut resumed = EventReader::open(&path, Some(checkpoint)).unwrap();
    assert!(resumed.next_event().unwrap().is_none());
    append(&path, &format!("\n{}", line("T-001", "done")));
    assert_eq!(resumed.next_event().unwrap().unwrap().0.kind, "done");
}

#[test]
#[cfg(unix)]
fn rejects_special_files_before_reading_or_blocking() {
    assert!(EventReader::open(Path::new("/dev/zero"), None).is_err());
    assert!(EventReader::from_offset(Path::new("/dev/null"), 0).is_err());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fifo");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    assert!(EventReader::open(&path, None).is_err());
}

#[test]
fn autopilot_actionable_kinds_and_single_line_format_are_shared() {
    for kind in ["blocked", "escalated", "done", "needs_me", "stuck"] {
        let event: Event = serde_json::from_value(serde_json::json!({
            "ts":"2026-10-15T00:00:00Z", "task":"T-041", "kind":"autopilot",
            "detail":{"to":kind}
        }))
        .unwrap();
        assert_eq!(actionable_kind(&event), Some(kind));
        assert_eq!(format_event_line(&event), format!("T-041 {kind}"));
    }
}
