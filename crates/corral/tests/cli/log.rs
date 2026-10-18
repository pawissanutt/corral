use super::*;
use corral_ops::{EntryClass, TimelineEntry};

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
