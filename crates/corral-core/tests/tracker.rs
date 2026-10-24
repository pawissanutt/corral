use corral_core::Store;

#[test]
fn pre_tracker_task_metadata_round_trips_byte_identically() {
    // Given a canonical task file written before tracker metadata existed.
    let original = "---\nschema_version: 2\ntask:\n  id: T-007\n  title: Ordinary task\n  status: backlog\n  priority: P2\n  created: 2026-10-17T12:00:00Z\n  updated: 2026-10-17T12:00:00Z\n---\nOriginal specification.\n";
    // When loaded and serialized without changing its contents.
    let task = Store::parse_task_markdown(original).unwrap();
    // Then absent tracker metadata introduces no bytes.
    assert!(!task.tracker);
    assert!(task.parent.is_none());
    assert!(task.source_digest.is_none());
    assert_eq!(Store::render_task_markdown(&task), original);
}

#[test]
fn tracker_metadata_survives_serialization() {
    // Given the same base task with explicit import metadata.
    let original = "---\nid: T-007\ntitle: Tracker\nstatus: backlog\ncreated: 2026-10-17T12:00:00Z\nupdated: 2026-10-17T12:00:00Z\n---\n";
    let mut task = Store::parse_task_markdown(original).unwrap();
    task.tracker = true;
    task.parent = Some("T-001".into());
    task.source_digest = Some("source-sha256".into());
    // When the task is serialized and reloaded.
    let saved = Store::parse_task_markdown(&Store::render_task_markdown(&task)).unwrap();
    // Then every new field is retained.
    assert_eq!(saved, task);
}
