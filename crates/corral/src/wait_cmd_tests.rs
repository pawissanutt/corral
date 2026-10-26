use super::*;
use clap::Parser;

#[test]
fn attention_batches_route_complete_membership_and_resume_without_replay() {
    // Given alpha's full routing membership, including a task outside sixteen display entries.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let alpha: Vec<_> = (1..=18).map(|n| format!("T-{n:03}")).collect();
    store.log_event(None, "attention_due", serde_json::json!({
        "project":"alpha","tasks":alpha,"count":18,
        "entries":alpha.iter().take(16).map(|id| serde_json::json!({"task":id})).collect::<Vec<_>>()
    })).unwrap();
    store
        .log_event(
            None,
            "attention_due",
            serde_json::json!({
                "project":"beta","tasks":["T-019","T-020"],"count":2,"entries":[]
            }),
        )
        .unwrap();
    // When task and project watchers read the same two batches.
    for (options, project) in [
        (vec!["--task", "T-018"], "alpha"),
        (vec!["--project", "alpha"], "alpha"),
        (vec!["--project", "beta"], "beta"),
    ] {
        let opts = args(&options);
        let mut reader = EventReader::from_offset(&store.dir().join("events.jsonl"), 0).unwrap();
        let mut matched = None;
        while let Some((event, cursor)) = reader.next_event().unwrap() {
            if opts.matches(&store, &event).unwrap() {
                matched = Some((event, cursor));
                break;
            }
        }
        let (event, cursor) = matched.unwrap();
        assert_eq!(event.detail["project"], project);
        // Then the returned cursor never replays its batch.
        let mut rearmed =
            EventReader::from_offset(&store.dir().join("events.jsonl"), cursor.offset).unwrap();
        while let Some((event, _)) = rearmed.next_event().unwrap() {
            assert!(!opts.matches(&store, &event).unwrap());
        }
    }
    let unmatched = args(&["--task", "T-999"]);
    let mut reader = EventReader::from_offset(&store.dir().join("events.jsonl"), 0).unwrap();
    while let Some((event, _)) = reader.next_event().unwrap() {
        assert!(!unmatched.matches(&store, &event).unwrap());
    }
}

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

#[test]
fn config_subscriptions_include_ui_changes_and_invalid_edits_without_task_filters() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    for kind in ["config_changed", "config_invalid"] {
        let mut reader = EventReader::open(&store.dir().join("events.jsonl"), None).unwrap();
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

#[test]
fn for_accepts_and_defaults_to_every_pm_actionable_kind_plus_config_invalid() {
    let all: Vec<String> = PM_ACTIONABLE_KINDS
        .iter()
        .chain(&["config_invalid"])
        .map(|k| k.to_string())
        .collect();
    assert_eq!(args(&[]).kinds, all);
    for kind in &all {
        assert_eq!(args(&["--for", kind]).kinds, std::slice::from_ref(kind));
    }
    for kind in ["question_answered", "pm_event_delivery"] {
        assert!(crate::Cli::try_parse_from(["corral", "wait", "--for", kind]).is_err());
    }
}

#[test]
fn default_wait_wakes_on_question_opened_and_rearmed_wait_skips_question_answered() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let path = store.dir().join("events.jsonl");
    let question = |extra: serde_json::Value| {
        let mut detail = serde_json::json!({
            "key": "T-001/R1/Q1", "revision": 1, "at": "2026-10-25T00:00:00Z"
        });
        detail
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        detail
    };
    // Given a default-kinds wait subscribed before the question opens. `follow` idles again only
    // after reading every appended event without a match, so a second idle is a failure:
    let mut reader = EventReader::open(&path, None).unwrap();
    let mut logged = false;
    let (opened, cursor) = follow(&store, &args(&[]), &mut reader, |_| {
        assert!(
            !std::mem::replace(&mut logged, true),
            "question_opened did not wake a default wait"
        );
        {
            let opened = question(serde_json::json!({"request_id": "r1", "blocking": true}));
            store
                .log_event(Some("T-001"), "question_opened", opened)
                .unwrap();
            let answered = question(serde_json::json!({"by": "pm"}));
            store
                .log_event(Some("T-001"), "question_answered", answered)
                .unwrap();
        }
    })
    .unwrap()
    .unwrap();
    // Then it returns on question_opened with its cursor:
    assert_eq!(opened.kind, "question_opened");
    assert_eq!(format_event_line(&opened), "T-001 question_opened");
    // And a wait re-armed at that cursor skips question_answered; the sentinel written at its
    // first idle proves the answer was read and not matched.
    let mut rearmed = EventReader::open(&path, Some(cursor)).unwrap();
    let mut sentinel = false;
    let (next, _) = follow(&store, &args(&[]), &mut rearmed, |_| {
        assert!(
            !std::mem::replace(&mut sentinel, true),
            "the sentinel done did not wake the re-armed wait"
        );
        {
            store
                .log_event(Some("T-002"), "done", serde_json::json!({}))
                .unwrap();
        }
    })
    .unwrap()
    .unwrap();
    assert_eq!(
        (next.task.as_deref(), next.kind.as_str()),
        (Some("T-002"), "done")
    );
}
