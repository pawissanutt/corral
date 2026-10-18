use super::*;
use corral_ops::{EntryClass, TimelineEntry};

#[test]
fn log_review_repeated_and_dual_role_waits_keep_every_endpoint() {
    // Given the independent review reproductions in an isolated CLI workspace.
    let ws = init_ws();
    std::fs::write(
        ws.root.join(".corral/events.jsonl"),
        include_str!("../../../corral-ops/tests/fixtures/timeline-wait-review.jsonl"),
    )
    .unwrap();
    // When querying the actual text and JSON commands.
    let repeated = ok(&ws.root, &["log", "205"]);
    let moved_closed = ok(&ws.root, &["log", "206"]);
    let moved_open = ok(&ws.root, &["log", "207"]);
    let moved_json = ok(&ws.root, &["log", "206", "--json"]);
    // Then repeated questions are resolved together and moved rows retain both roles.
    assert!(!repeated.contains("(open)"), "{repeated}");
    let reply = repeated
        .lines()
        .find(|line| line.contains("replied"))
        .unwrap();
    assert_eq!(reply.matches('┘').count(), 2, "{repeated}");
    for (text, open) in [(&moved_closed, false), (&moved_open, true)] {
        let moved = text.lines().find(|line| line.contains("moved")).unwrap();
        assert!(moved.contains('┘') && moved.contains('┐'), "{moved}");
        assert_eq!(moved.contains("(open)"), open, "{moved}");
    }
    let entries: Vec<TimelineEntry> = moved_json
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(entries[1].wait.as_ref().unwrap().opened_at, entries[0].at);
}

#[test]
fn log_renders_paired_brackets_and_keeps_unrelated_operation_open() {
    // Given the acceptance fixture in an isolated CLI workspace.
    let ws = init_ws();
    std::fs::write(
        ws.root.join(".corral/events.jsonl"),
        include_str!("../../../corral-ops/tests/fixtures/timeline-waits.jsonl"),
    )
    .unwrap();
    // When querying both text and JSON surfaces.
    let paired = ok(&ws.root, &["log", "201"]);
    let open = ok(&ws.root, &["log", "204"]);
    let json = ok(&ws.root, &["log", "201", "--json"]);
    // Then bracket endpoints, duration, open age, and machine-readable metadata agree.
    assert!(paired.contains("┐ waited 1h 05m"), "{paired}");
    assert!(paired.contains("┘ waited 1h 05m"), "{paired}");
    let open_row = open.lines().find(|row| row.contains("blocked")).unwrap();
    assert!(open_row.contains("┐ waiting "), "{open}");
    assert!(open_row.ends_with(" (open)"), "{open}");
    let entries: Vec<TimelineEntry> = json
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let wait = entries[1].wait.as_ref().unwrap();
    assert_eq!(wait.duration, std::time::Duration::from_secs(65 * 60));
    assert!(!wait.open);
}

fn fixture() -> Ws {
    let ws = init_ws();
    std::fs::write(
        ws.root.join(".corral/events.jsonl"),
        include_str!("../../../corral-ops/tests/fixtures/task-history.jsonl"),
    )
    .unwrap();
    ws
}

#[test]
fn log_text_orders_lifecycle_review_and_run_with_actor_via_columns() {
    let ws = fixture();
    let out = ok(&ws.root, &["log", "123"]);
    let mut lines = out.lines();
    assert_eq!(
        lines.next().unwrap().split_whitespace().collect::<Vec<_>>(),
        ["TIME", "KIND", "ACTOR", "VIA", "RUN", "SUMMARY"]
    );
    let rows: Vec<_> = lines
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .collect();
    assert_eq!(
        rows.iter().map(|row| row[2]).collect::<Vec<_>>(),
        [
            "created",
            "edited",
            "moved",
            "dispatched",
            "needs_me",
            "replied",
            "gate",
            "verdict",
            "approved",
            "commented",
            "blocked",
            "progress_updated",
            "plan_verdict"
        ]
    );
    for (kind, actor, via) in [
        ("created", "owner", "?"),
        ("moved", "pm", "?"),
        ("replied", "owner", "?"),
        ("needs_me", "agent", "run:R1"),
        ("gate", "system", "run:R1"),
        ("verdict", "reviewer", "run:R1"),
        ("commented", "pm", "pm"),
        ("blocked", "?", "?"),
    ] {
        let row = rows.iter().find(|row| row[2] == kind).unwrap();
        assert_eq!(row[3], actor, "{kind}");
        assert_eq!(row[4], via, "{kind}");
    }
    assert!(!out.contains("future_kind"));
    assert!(!out.contains("OTHER TASK"));
    assert!(!out.contains("GLOBAL"));
    let all = ok(&ws.root, &["log", "T-123", "--all"]);
    assert_eq!(all.lines().count(), 16);
    assert!(all.contains("reply_delivery"));
    assert!(all.contains("future_kind"));
    assert!(!all.contains("OTHER TASK"));
    assert!(!all.contains("GLOBAL"));
}

#[test]
fn log_json_is_typed_jsonl_with_identical_filtering() {
    let ws = fixture();
    for (flags, count) in [
        (vec!["log", "t-123", "--json"], 13),
        (vec!["log", "123", "--all", "--json"], 15),
    ] {
        let out = ok(&ws.root, &flags);
        let entries: Vec<TimelineEntry> = out
            .lines()
            .map(|line| serde_json::from_str(line).expect("one TimelineEntry per line"))
            .collect();
        assert_eq!(entries.len(), count);
        assert!(entries.windows(2).all(|pair| pair[0].at <= pair[1].at));
        assert!(
            entries
                .iter()
                .all(|e| e.event.task.as_deref() == Some("T-123"))
        );
        if count == 15 {
            let unknown = entries.last().unwrap();
            assert_eq!(unknown.kind, "future_kind");
            assert_eq!(unknown.summary, unknown.kind);
            assert_eq!(unknown.class, EntryClass::Noise);
        } else {
            assert!(entries.iter().all(|e| e.class != EntryClass::Noise));
        }
    }
    assert_eq!(ok(&ws.root, &["log", "999", "--json"]), "");
    assert!(!corral(&ws.root, &["log", "bad"]).status.success());
}

#[test]
fn log_preserves_producer_questions_errors_and_approval_semantics() {
    let ws = init_ws();
    std::fs::write(
        ws.root.join(".corral/events.jsonl"),
        include_str!("../../../corral-ops/tests/fixtures/timeline-producer-summaries.jsonl"),
    )
    .unwrap();
    let expected = [
        "jsonl or sqlite? Action: Choose a storage format.",
        "provider_stall Rate limit reached. Retry after quota resets.",
        "kept branch corral/T-125 at a1b2c3d (not merged)",
        "landed b2c3d4e (ff)",
        "landed c3d4e5f (rebase)",
    ];
    let text = ok(&ws.root, &["log", "125"]);
    let rows: Vec<_> = text.lines().skip(1).collect();
    assert_eq!(rows.len(), expected.len());
    for (row, summary) in rows.iter().zip(expected) {
        assert!(row.contains(summary), "{row}");
    }
    assert!(!text.contains("OTHER TASK"));
    let json = ok(&ws.root, &["log", "125", "--json"]);
    let entries: Vec<TimelineEntry> = json
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        entries
            .iter()
            .map(|e| e.summary.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    assert!(
        entries
            .iter()
            .all(|e| e.event.task.as_deref() == Some("T-125"))
    );
    assert_eq!(
        entries[0].event.detail["requested_action"],
        "Choose a\nstorage format."
    );
    assert_eq!(
        entries[1].event.detail["message"],
        "Rate limit reached.\nRetry after quota resets."
    );
    assert_eq!(entries[2].event.detail["strategy"], "keep_branch");
}
