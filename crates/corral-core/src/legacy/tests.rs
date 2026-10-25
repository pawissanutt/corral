use std::path::Path;

use super::*;

const FLAT: &str = "---\nid: T-007\ntitle: Flat legacy task\nstatus: backlog\ncreated: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\n---\nBody line\n";

#[test]
fn a_flat_legacy_task_parses_and_renders_as_schema_two() {
    let task = parse_task(FLAT, Path::new("T-007.md")).unwrap();
    assert_eq!(task.id, "T-007");
    assert_eq!(task.body, "Body line\n");
    assert_eq!(task.path, Path::new("T-007.md"));
    let rendered = render_task(&task);
    assert!(
        rendered.starts_with("---\nschema_version: 2\n"),
        "{rendered}"
    );
    assert_eq!(
        parse_task(&rendered, Path::new("")).unwrap().title,
        task.title
    );
}

#[test]
fn an_owner_idea_without_origin_is_owned_by_the_owner() {
    let text = FLAT.replace("status: backlog", "status: idea");
    let task = parse_task(&text, Path::new("")).unwrap();
    assert_eq!(task.origin, Some(crate::Origin::Owner));
}

#[test]
fn malformed_frontmatter_is_refused_naming_the_file() {
    for text in ["no frontmatter", "---\nid: T-1\n"] {
        let error = parse_task(text, Path::new("bad.md")).unwrap_err();
        assert!(format!("{error:#}").contains("bad.md"), "{error:#}");
    }
}

#[test]
fn recovery_records_require_schema_three() {
    let text = "---\nschema_version: 2\ntask:\n  id: T-001\n  title: t\n  status: backlog\n  created: 2026-01-01T00:00:00Z\n  updated: 2026-01-01T00:00:00Z\nrecoveries: []\n---\n";
    let error = parse_task_record(text, Path::new("")).unwrap_err();
    assert!(format!("{error:#}").contains("schema 3"), "{error:#}");
    let unsupported = text.replace("schema_version: 2", "schema_version: 9");
    assert!(parse_task_record(&unsupported, Path::new("")).is_err());
}

#[test]
fn a_record_without_recoveries_round_trips_through_schema_three() {
    let task = parse_task(FLAT, Path::new("")).unwrap();
    let record = TaskRecord {
        task,
        recoveries: Vec::new(),
    };
    let text = render_task_record(&record).unwrap();
    assert!(text.starts_with("---\nschema_version: 3\n"), "{text}");
    let mut back = parse_task_record(&text, Path::new("")).unwrap();
    back.task.path = record.task.path.clone();
    assert_eq!(back, record);
}
