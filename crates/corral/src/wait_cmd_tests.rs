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
    let mut reader = store.events_since(None).unwrap();
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
        "seq":123, "ts":"2026-10-15T00:00:00Z", "task":"T-001", "kind":"done",
        "detail":{"run":"R1","added":3,"removed":1,"reason":"a\nb"}
    }))
    .unwrap();
    let cursor = EventCursor {
        seq: 123,
        ..EventCursor::default()
    };
    let json = output(&args(&["--json", "--print-cursor"]), &event, &cursor).unwrap();
    assert_eq!(json.lines().count(), 1);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["seq"], 123);
    assert_eq!(value["cursor"], serde_json::json!({"seq": 123}));
    assert!(value.get("after_offset").is_none());
    assert_eq!(value["detail"], event.detail);
    let human = output(&args(&["--print-cursor"]), &event, &cursor).unwrap();
    assert_eq!(human.lines().count(), 1);
    let (line, encoded) = human.split_once(" cursor=").unwrap();
    assert_eq!(line, "T-001 done R1 +3/-1: a b");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(encoded).unwrap(),
        serde_json::json!({"seq": cursor.seq})
    );
}

#[test]
fn zero_timeout_returns_without_reading_or_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let mut reader = store.events_since(None).unwrap();
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
        vec!["--after", "-1"],
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

#[test]
fn cli_rejects_byte_offsets_and_accepts_sequence_cursors() {
    // Given a former byte-offset argument, with or without a value.
    for extra in [vec!["--after-offset", "123"], vec!["--after-offset"]] {
        let mut words = vec!["corral", "wait"];
        words.extend(extra);
        // When parsing the removed flag.
        let error = crate::Cli::try_parse_from(words).err().unwrap();
        // Then the parser rejects it and names the replacement flag.
        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(error.to_string().contains("use --after SEQ"));
    }
    assert_eq!(args(&["--after", "123"]).after, Some(123));
}

#[test]
fn config_subscriptions_include_ui_changes_and_invalid_edits_without_task_filters() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    for kind in ["config_changed", "config_invalid"] {
        let mut reader = store.events_since(None).unwrap();
        let opts = args(&["--for", kind]);
        let found = follow(&store, &opts, &mut reader, |_| {
            store
                .log_event(None, kind, serde_json::json!({"pm_relevant": false}))
                .unwrap();
        })
        .unwrap()
        .unwrap()
        .0;
        assert_eq!(found.kind, kind);
        assert!(args(&[]).matches(&store, &found).unwrap());
        assert!(!args(&["--task", "T-001"]).matches(&store, &found).unwrap());
        assert!(!args(&["--project", "app"]).matches(&store, &found).unwrap());
    }
}
