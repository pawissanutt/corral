use super::*;
use serde_json::json;

fn fixture() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    std::fs::write(
        store.dir().join("events.jsonl"),
        include_str!("../tests/fixtures/timeline-waits.jsonl"),
    )
    .unwrap();
    (dir, store)
}

#[test]
fn replied_has_sixty_five_minute_wait_from_fixture() {
    // Given an owner reply 65 minutes after the question.
    let (_dir, store) = fixture();
    // When the read-time timeline is built.
    let entries = timeline(&store, "201", TimelineFilter::Relevant).unwrap();
    // Then both bracket endpoints identify the elapsed wait, including the closing JSON.
    let wait = entries[1].wait.as_ref().unwrap();
    assert_eq!(wait.opened_at, entries[0].at);
    assert_eq!(wait.duration, Duration::from_secs(65 * 60));
    assert_eq!(wait.opened_by_kind, "needs_me");
    assert!(!wait.open);
    assert_eq!(
        entries[0].wait_label(entries[1].at).as_deref(),
        Some("┐ waited 1h 05m")
    );
    assert_eq!(
        entries[1].wait_label(entries[1].at).as_deref(),
        Some("┘ waited 1h 05m")
    );
    assert_eq!(
        std::fs::read_to_string(store.dir().join("events.jsonl")).unwrap(),
        include_str!("../tests/fixtures/timeline-waits.jsonl")
    );
}

#[test]
fn interleaved_runs_close_their_own_waits() {
    // Given two overlapping waits whose replies arrive in reverse order.
    let (_dir, store) = fixture();
    // When pairing the timeline.
    let entries = timeline(&store, "202", TimelineFilter::All).unwrap();
    // Then R2 closes its ten-minute wait, not R1's forty-minute wait.
    for (index, opener, minutes) in [(2, 1, 10), (3, 0, 40)] {
        let wait = entries[index].wait.as_ref().unwrap();
        assert_eq!(wait.opened_at, entries[opener].at);
        assert_eq!(wait.duration, Duration::from_secs(minutes * 60));
        assert_eq!(entries[index].run, entries[opener].run);
    }
}

#[test]
fn operation_wait_requires_a_matching_operation_on_the_resolution() {
    // Given a post-land failure, an unrelated reply and another operation's completion.
    let (_dir, store) = fixture();
    // When reading its eventual matching approval.
    let entries = timeline(&store, "203", TimelineFilter::Relevant).unwrap();
    // Then neither unrelated event closes the operation.
    assert!(entries[1].wait.is_none());
    assert!(entries[2].wait.is_none());
    let wait = entries[3].wait.as_ref().unwrap();
    assert_eq!(wait.opened_at, entries[0].at);
    assert_eq!(wait.duration, Duration::from_secs(20 * 60));
    assert!(!wait.open);
}

#[test]
fn unclosed_operation_renders_open_with_current_age() {
    // Given an unresolved post-land operation and a fixed render clock.
    let (_dir, store) = fixture();
    let now = "2026-10-15T12:10:00Z".parse().unwrap();
    // When rendering the wait label from a lazily cached timeline.
    let entries = timeline(&store, "204", TimelineFilter::Relevant).unwrap();
    let label = entries[0].wait_label(now);
    // Then the age is based on the render clock, not the cached query clock.
    assert!(entries[0].wait.as_ref().unwrap().open);
    assert!(entries[1].wait.is_none());
    assert_eq!(label.as_deref(), Some("┐ waiting 2h 10m (open)"));
}

#[test]
fn every_design_opener_and_resolution_pairs_including_legacy_runs() {
    for (kind, detail) in [
        ("needs_me", json!({})),
        ("blocked", json!({})),
        ("escalated", json!({})),
        ("plan_awaiting_owner", json!({})),
        ("review_escalated", json!({})),
        ("moved", json!({"from":"running","to":"needs_me"})),
    ] {
        for (close, resolution) in [
            ("replied", json!({})),
            ("unstuck", json!({"run":"R1"})),
            ("approved", json!({})),
            ("changes_requested", json!({})),
            ("discarded", json!({})),
            ("done", json!({})),
            ("owner_decision", json!({})),
            ("plan_verdict", json!({})),
            ("moved", json!({"from":"needs_me","to":"running"})),
            ("moved", json!({"from":"review","to":"ready"})),
        ] {
            // Given each designated opener/resolver pair, without mandatory run IDs.
            let (_dir, store) = fixture();
            let events = [
                json!({"ts":"2026-10-15T10:00:00Z","task":"T-205","kind":kind,"detail":detail}),
                json!({"ts":"2026-10-15T10:01:00Z","task":"T-205","kind":close,"detail":resolution}),
            ];
            std::fs::write(
                store.dir().join("events.jsonl"),
                events.iter().map(|e| format!("{e}\n")).collect::<String>(),
            )
            .unwrap();
            // When querying the default filtered view.
            let entries = timeline(&store, "205", TimelineFilter::Relevant).unwrap();
            // Then the resolution closes the one-minute wait and remains visible.
            assert_eq!(entries.len(), 2, "{kind} -> {close}");
            let wait = entries[1].wait.as_ref().unwrap();
            assert_eq!(wait.opened_by_kind, kind);
            assert_eq!(wait.duration, Duration::from_secs(60));
            assert!(!wait.open);
        }
    }
}

#[test]
fn equal_timestamp_waits_keep_append_order_and_zero_duration() {
    // Given a wait and reply appended at the same timestamp.
    let (_dir, store) = fixture();
    let at = "2026-10-15T10:00:00Z";
    let rows = ["needs_me", "replied"]
        .map(|kind| {
            format!(
                "{}\n",
                json!({"ts":at,"task":"T-205","kind":kind,"detail":{}})
            )
        })
        .concat();
    std::fs::write(store.dir().join("events.jsonl"), rows).unwrap();
    // When pairing.
    let entries = timeline(&store, "205", TimelineFilter::All).unwrap();
    // Then the endpoints are distinct even though their timestamps match.
    assert_eq!(entries[1].wait.as_ref().unwrap().duration, Duration::ZERO);
    assert_eq!(
        entries[0].wait_label(entries[0].at).as_deref(),
        Some("┐ waited 0m")
    );
    assert_eq!(
        entries[1].wait_label(entries[1].at).as_deref(),
        Some("┘ waited 0m")
    );
}

fn review_fixture() -> (tempfile::TempDir, Store) {
    let (dir, store) = fixture();
    std::fs::write(
        store.dir().join("events.jsonl"),
        include_str!("../tests/fixtures/timeline-wait-review.jsonl"),
    )
    .unwrap();
    (dir, store)
}

#[test]
fn review_repeated_openers_close_on_the_same_next_resolution() {
    // Given repeated R1 questions followed by one reply and a later done event.
    let (_dir, store) = review_fixture();
    // When building the timeline.
    let entries = timeline(&store, "205", TimelineFilter::Relevant).unwrap();
    // Then each opener closes at the reply, not at a later unrelated resolution.
    for (index, minutes) in [(0, 10), (1, 5)] {
        let wait = entries[index].wait.as_ref().unwrap();
        assert!(!wait.open, "opener {index} is already resolved");
        assert_eq!(wait.duration, Duration::from_secs(minutes * 60));
        assert!(
            !entries[index]
                .wait_label(entries[2].at)
                .unwrap()
                .contains("(open)")
        );
    }
    assert!(
        entries[3].wait.is_none(),
        "done must not consume a leftover wait"
    );
    assert_eq!(entries[2].additional_waits.len(), 1);
    assert_eq!(entries[2].additional_waits[0].opened_at, entries[1].at);
    assert_eq!(
        entries[2].additional_waits[0].duration,
        Duration::from_secs(5 * 60)
    );
    assert_eq!(
        entries[2]
            .wait_label(entries[2].at)
            .unwrap()
            .matches('┘')
            .count(),
        2
    );
}

#[test]
fn review_moved_keeps_both_closed_wait_roles() {
    // Given a move that resolves one question and starts another resolved by a reply.
    let (_dir, store) = review_fixture();
    // When building the timeline.
    let entries = timeline(&store, "206", TimelineFilter::Relevant).unwrap();
    // Then the moved row retains its incoming resolution and both rendered endpoints.
    let closed = entries[1].wait.as_ref().unwrap();
    assert_eq!(closed.opened_at, entries[0].at);
    assert_eq!(closed.opened_by_kind, "needs_me");
    assert_eq!(closed.duration, Duration::from_secs(5 * 60));
    assert!(!closed.open);
    let opened = entries[1].opened_wait.as_ref().unwrap();
    assert_eq!(opened.opened_at, entries[1].at);
    assert_eq!(opened.opened_by_kind, "moved");
    assert_eq!(opened.duration, Duration::from_secs(5 * 60));
    assert!(!opened.open);
    let label = entries[1].wait_label(entries[2].at).unwrap();
    assert_eq!(label.matches('┘').count(), 1, "{label}");
    assert_eq!(label.matches('┐').count(), 1, "{label}");
    assert!(!label.contains("(open)"), "{label}");
}

#[test]
fn review_moved_keeps_resolution_while_new_wait_is_open() {
    // Given a dual-role move without a later reply.
    let (_dir, store) = review_fixture();
    let now = "2026-10-15T12:10:00Z".parse().unwrap();
    // When rendering its timeline at a fixed clock.
    let entries = timeline(&store, "207", TimelineFilter::Relevant).unwrap();
    // Then the closed five-minute wait survives alongside the new open interval.
    let closed = entries[1].wait.as_ref().unwrap();
    assert_eq!(closed.opened_at, entries[0].at);
    assert!(!closed.open);
    let opened = entries[1].opened_wait.as_ref().unwrap();
    assert_eq!(opened.opened_at, entries[1].at);
    assert_eq!(opened.opened_by_kind, "moved");
    assert!(opened.open);
    let label = entries[1].wait_label(now).unwrap();
    assert!(label.contains('┘') && label.contains('┐'), "{label}");
    assert!(
        label.contains("5m") && label.contains("2h 05m") && label.contains("(open)"),
        "{label}"
    );
}

#[test]
fn review_repeated_openers_preserve_run_and_operation_isolation() {
    // Given repeated questions, another run, and repeated operations X and Y.
    let (_dir, store) = review_fixture();
    // When replying to R1, resolving X, then unsticking R2.
    let entries = timeline(&store, "208", TimelineFilter::Relevant).unwrap();
    // Then only operation Y remains open; each other opener uses its first eligible resolution.
    for (index, minutes) in [(0, 10), (1, 9), (2, 18), (3, 12), (4, 11)] {
        let wait = entries[index].wait.as_ref().unwrap();
        assert!(!wait.open, "opener {index} was resolved");
        assert_eq!(
            wait.duration,
            Duration::from_secs(minutes * 60),
            "opener {index}"
        );
    }
    assert!(entries[5].wait.as_ref().unwrap().open);
    assert_eq!(entries[6].additional_waits[0].opened_at, entries[1].at);
    assert_eq!(entries[7].wait.as_ref().unwrap().opened_at, entries[3].at);
    assert_eq!(entries[7].additional_waits[0].opened_at, entries[4].at);
}
