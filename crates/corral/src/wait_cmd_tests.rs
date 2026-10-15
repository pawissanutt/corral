use super::*;
use clap::Parser;

fn args(extra: &[&str]) -> WaitArgs {
    let mut words = vec!["corral", "wait"];
    words.extend_from_slice(extra);
    match crate::Cli::try_parse_from(words).unwrap().cmd {
        Some(crate::Cmd::Wait(args)) => args,
        _ => panic!("expected wait"),
    }
}

#[test]
fn first_event_after_subscription_ignores_history_and_nonmatching_events() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    store
        .log_event(Some("T-001"), "done", serde_json::json!({}))
        .unwrap();
    let mut reader = EventReader::open(&store.dir().join("events.jsonl"), None).unwrap();
    // The idle callback is the subscription handshake, not a timed race.
    let mut subscribed = false;
    let found = follow(&store, &args(&[]), &mut reader, |_| {
        assert!(!subscribed);
        subscribed = true;
        store
            .log_event(Some("T-002"), "created", serde_json::json!({}))
            .unwrap();
        store
            .log_event(Some("T-003"), "done", serde_json::json!({}))
            .unwrap();
        store
            .log_event(Some("T-004"), "stuck", serde_json::json!({}))
            .unwrap();
    })
    .unwrap()
    .unwrap();
    assert!(subscribed);
    assert_eq!(found.0.task.as_deref(), Some("T-003"));
    assert_eq!(reader.next_event().unwrap().unwrap().0.kind, "stuck");
}

#[test]
fn task_project_since_and_autopilot_filters_compose() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let task = store
        .create("target", Some("app"), corral_core::Status::Review, "")
        .unwrap();
    let opts = args(&[
        "--for",
        "blocked,escalated",
        "--task",
        &task.id,
        "--project",
        "app",
        "--since",
        "2026-10-15T00:00:00Z",
    ]);
    let make = |id: &str, ts: &str, to: &str| -> Event {
        serde_json::from_value(serde_json::json!({
            "ts":ts, "task":id, "kind":"autopilot", "detail":{"to":to}
        }))
        .unwrap()
    };
    let event = make(&task.id, "2026-10-15T00:00:00Z", "blocked");
    assert!(opts.matches(&store, &event).unwrap());
    assert!(
        !opts
            .matches(&store, &make("T-999", "2026-10-15T00:00:00Z", "blocked"))
            .unwrap()
    );
    assert!(
        !opts
            .matches(&store, &make(&task.id, "2026-10-14T00:00:00Z", "blocked"))
            .unwrap()
    );
    assert!(
        !opts
            .matches(&store, &make(&task.id, "2026-10-15T00:00:00Z", "reviewing"))
            .unwrap()
    );
    let mut wrong_project = opts;
    wrong_project.project = Some("other".into());
    assert!(!wrong_project.matches(&store, &event).unwrap());
}

#[test]
fn output_preserves_event_and_adds_resumable_cursor() {
    let event: Event = serde_json::from_value(serde_json::json!({
        "ts":"2026-10-15T00:00:00Z", "task":"T-001", "kind":"done",
        "detail":{"run":"R1","added":3,"removed":1,"reason":"a\nb"}
    }))
    .unwrap();
    let cursor = EventCursor {
        offset: 123,
        ..EventCursor::default()
    };
    let json = output(&args(&["--json", "--print-cursor"]), &event, &cursor).unwrap();
    assert_eq!(json.lines().count(), 1);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["after_offset"], 123);
    assert_eq!(value["cursor"]["offset"], 123);
    assert_eq!(value["detail"], event.detail);
    let human = output(&args(&["--print-cursor"]), &event, &cursor).unwrap();
    assert_eq!(human.lines().count(), 1);
    let (line, encoded) = human.split_once(" cursor=").unwrap();
    assert_eq!(line, "T-001 done R1 +3/-1: a b");
    assert_eq!(
        serde_json::from_str::<EventCursor>(encoded).unwrap(),
        cursor
    );
}

#[test]
fn zero_timeout_returns_without_reading_or_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let mut reader = EventReader::open(&store.dir().join("events.jsonl"), None).unwrap();
    assert!(
        follow(&store, &args(&["--timeout", "0ms"]), &mut reader, |_| {
            panic!("zero timeout must not wait")
        })
        .unwrap()
        .is_none()
    );
}

#[test]
fn cli_rejects_invalid_filters_and_durations() {
    for extra in [
        vec!["--for", "wat"],
        vec!["--since", "yesterday"],
        vec!["--timeout", "1"],
        vec!["--timeout", "18446744073709551615d"],
        vec!["--after-offset", "-1"],
    ] {
        let mut words = vec!["corral", "wait"];
        words.extend(extra);
        assert!(crate::Cli::try_parse_from(words).is_err());
    }
    assert_eq!(
        args(&["--timeout", "30m"]).timeout,
        Some(Duration::from_secs(1800))
    );
}
