use super::*;
use serde_json::json;

#[test]
fn fixture_filters_orders_attributes_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    std::fs::write(
        store.dir().join("events.jsonl"),
        include_str!("../tests/fixtures/task-history.jsonl"),
    )
    .unwrap();

    let entries = timeline(&store, "123", TimelineFilter::Relevant).unwrap();
    assert_eq!(entries.len(), 13);
    assert!(entries.windows(2).all(|pair| pair[0].at <= pair[1].at));
    assert!(entries.iter().all(|entry| {
        entry.event.task.as_deref() == Some("T-123")
            && entry.class != EntryClass::Noise
            && entry.wait.is_none()
            && !entry.summary.contains(['\n', '\r', '\t'])
    }));
    for (kind, actor) in [
        ("created", Some("owner")),
        ("moved", Some("pm")),
        ("replied", Some("owner")),
        ("commented", Some("pm")),
        ("blocked", None),
    ] {
        let entry = entries.iter().find(|entry| entry.kind == kind).unwrap();
        assert_eq!(entry.actor.as_deref(), actor, "{kind}");
    }
    let needs_me = entries.iter().find(|e| e.kind == "needs_me").unwrap();
    assert_eq!(needs_me.run.as_deref(), Some("R1"));
    assert_eq!(needs_me.via.as_deref(), Some("run:R1"));
    assert!(entries[0].via.is_none());
    assert_eq!(entries[1].summary, "body (+6 lines), title");
    let gate = entries.iter().find(|e| e.kind == "gate").unwrap();
    assert_eq!(gate.class, EntryClass::Review);
    assert_eq!(gate.summary, "pass");
    for entry in &entries {
        let line = serde_json::to_string(entry).unwrap();
        assert_eq!(
            serde_json::from_str::<TimelineEntry>(&line).unwrap(),
            *entry
        );
    }

    let all = timeline(&store, "t-123", TimelineFilter::All).unwrap();
    assert_eq!(all.len(), 15);
    assert_eq!(all.last().unwrap().summary, "future_kind");
    assert_eq!(all.last().unwrap().class, EntryClass::Noise);
    assert!(all.iter().all(|e| e.event.task.as_deref() == Some("T-123")));
    assert!(
        all.iter()
            .find(|e| e.kind == "reply_delivery")
            .unwrap()
            .via
            .is_none()
    );
}

#[test]
fn classification_matches_design_including_prefix_families() {
    for (class, kinds) in [
        (
            EntryClass::Lifecycle,
            "created edited moved queued dequeued idea_decided idea_merged owner_decision dispatched needs_me blocked escalated replied unstuck commented approved changes_requested discarded done reverted superseded",
        ),
        (
            EntryClass::Review,
            "gate_started gate review_started verdict autopilot plan_submitted plan_future",
        ),
        (
            EntryClass::Run,
            "profile_switched checkpoint_saved checkpoint_future progress_updated provider_stall provider_future idle_no_report",
        ),
        (
            EntryClass::Noise,
            "reply_delivery lesson dashboard future_kind",
        ),
    ] {
        for kind in kinds.split_whitespace() {
            assert_eq!(classify(kind), class, "{kind}");
        }
    }
}

#[test]
fn summaries_use_producer_payloads_and_stay_on_one_line() {
    for (kind, detail, expected) in [
        (
            "created",
            json!({"status":"idea","title":"A\nB"}),
            "idea \"A B\"",
        ),
        (
            "moved",
            json!({"from":"idea","to":"ready","reason":"go"}),
            "idea -> ready go",
        ),
        ("queued", json!({"reason":"slots full"}), "slots full"),
        ("dequeued", json!({}), "dequeued"),
        (
            "idea_decided",
            json!({"action":"defer","reason":"later"}),
            "defer later",
        ),
        ("idea_merged", json!({"into":"T-042"}), "into T-042"),
        (
            "owner_decision",
            json!({"action":"drop","reason":"duplicate"}),
            "drop duplicate",
        ),
        (
            "dispatched",
            json!({"run":"R1","model":"claude/opus","pane":"w3:2"}),
            "R1 claude/opus, pane w3:2",
        ),
        ("needs_me", json!({"note":"choose\njsonl"}), "choose jsonl"),
        (
            "blocked",
            json!({"reason":"install failed"}),
            "install failed",
        ),
        ("escalated", json!({"reason":"budget"}), "budget"),
        ("replied", json!({"text":"jsonl"}), "jsonl"),
        ("unstuck", json!({"pid":42}), "stopped pid 42"),
        (
            "commented",
            json!({"comment":{"text":"keep\nit"}}),
            "keep it",
        ),
        (
            "approved",
            json!({"sha":"abcdef0","strategy":"fast_forward"}),
            "landed abcdef0 (ff)",
        ),
        ("changes_requested", json!({"text":"fix it"}), "fix it"),
        ("discarded", json!({"reason":"duplicate"}), "duplicate"),
        (
            "done",
            json!({"files":2,"added":4,"removed":1}),
            "2 files, +4 -1",
        ),
        (
            "reverted",
            json!({"landed_sha":"abcdef0"}),
            "reverted abcdef0",
        ),
        ("superseded", json!({"run":"R1"}), "superseded R1"),
        (
            "gate_started",
            json!({"commands":["test"],"pane":"w:p"}),
            "1 commands, pane w:p",
        ),
        (
            "gate",
            json!({"result":"fail","failed_cmd":"test"}),
            "fail test",
        ),
        (
            "review_started",
            json!({"reviewer":"opus","pane":"w:p"}),
            "opus pane w:p",
        ),
        (
            "verdict",
            json!({"verdict":"approve","notes":"good"}),
            "approve good",
        ),
        (
            "autopilot",
            json!({"from":"gating","to":"reviewing"}),
            "gating -> reviewing",
        ),
        (
            "profile_switched",
            json!({"from":"fast","to":"strong"}),
            "fast -> strong",
        ),
        (
            "checkpoint_saved",
            json!({"revision":3}),
            "checkpoint_saved revision 3",
        ),
        (
            "provider_stall",
            json!({"reason":"quota"}),
            "provider_stall quota",
        ),
        ("idle_no_report", json!({"note":"idle"}), "idle"),
        ("plan_submitted", json!({"plan":{"id":"P1"}}), "plan P1"),
        ("plan_policy", json!({"required":true}), "required: true"),
        (
            "plan_decomposed",
            json!({"children":["T-042"]}),
            "1 children",
        ),
        (
            "plan_verdict",
            json!({"review":{"verdict":"approve","notes":"good"}}),
            "approve good",
        ),
        ("future_kind", json!({"reason":"hidden"}), "future_kind"),
    ] {
        let event = Event {
            ts: Timestamp::UNIX_EPOCH,
            task: Some("T-123".into()),
            kind: kind.into(),
            actor: None,
            via: None,
            detail,
        };
        assert_eq!(summarize(&event), expected, "{kind}");
    }
}

#[test]
fn empty_missing_and_invalid_queries_are_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    assert!(
        timeline(&store, "123", TimelineFilter::All)
            .unwrap()
            .is_empty()
    );
    assert!(timeline(&store, "bad", TimelineFilter::All).is_err());
    // Equal timestamps retain the original log order.
    store.log_event(Some("T-123"), "queued", json!({})).unwrap();
    let mut first = store.events().unwrap().remove(0);
    let mut second = first.clone();
    first.kind = "blocked".into();
    second.kind = "replied".into();
    std::fs::write(
        store.dir().join("events.jsonl"),
        format!(
            "{}\n{}\n",
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap()
        ),
    )
    .unwrap();
    let entries = timeline(&store, "123", TimelineFilter::All).unwrap();
    assert_eq!(entries[0].kind, "blocked");
    assert_eq!(entries[1].kind, "replied");
    std::fs::remove_file(store.dir().join("events.jsonl")).unwrap();
    std::fs::create_dir(store.dir().join("events.jsonl")).unwrap();
    assert!(timeline(&store, "123", TimelineFilter::All).is_err());
}
