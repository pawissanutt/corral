use super::super::durable::wait;
use super::*;
use corral_protocol::{
    DaemonState, LoopFacts, PmState,
    board::{BoardCursor, BoardGroup, BoardNow, RailGlyph, RailStep},
    rows::{InboxKind, InboxState, PlanState},
    subscription::RowsDelta,
};
use nsh_core::prelude::signal;
use serde_json::json;

fn reset() {
    wait(durable::transaction(|records| {
        records.clear();
        Ok(())
    }))
    .unwrap();
}

fn session(device: &str) -> AppSession {
    serde_json::from_value(json!({
        "device": {
            "id": device, "name": "Saved device", "platform": "native", "app_version": "test",
            "enrolled_at": "2026-11-15T10:00:00Z", "revoked_at": null
        },
        "authority": {"reply": true, "act": true, "assistant": true, "assistant_act": true, "admin": true},
        "seen": {"through": 0}, "server_time": "2026-11-15T10:00:00Z"
    })).unwrap()
}

fn live_feed(store: &str) -> Signal<Feed> {
    signal(Feed {
        store: Some(store.into()),
        clock: "10:01".into(),
        synced: Some("10:00".into()),
        capabilities: BTreeSet::from(["board.v1".into(), "inbox.v1".into(), "plans.v1".into()]),
        ..Feed::default()
    })
}

fn board_row(id: &str, title: &str) -> BoardRow {
    BoardRow {
        id: id.into(),
        title: title.into(),
        project: None,
        plan: None,
        goal: None,
        group: BoardGroup::Queued,
        rail: std::array::from_fn(|_| RailStep {
            glyph: RailGlyph::Pending,
            sha7: None,
            at: None,
        }),
        now: BoardNow {
            code: "ready".into(),
            text: "Ready".into(),
            since: None,
        },
        age_since: None,
        held_reason: None,
        questions: 0,
        attention: vec![],
        agent: None,
        cost_usd: None,
        cost_basis: None,
        last_event: None,
        rev: 1,
    }
}

fn inbox_row(id: &str, title: &str) -> InboxRow {
    InboxRow {
        id: id.into(),
        kind: InboxKind::WorkerQuestion,
        task: Some("T-2".into()),
        plan: None,
        project: None,
        title: title.into(),
        from: "worker".into(),
        asked_at: None,
        needed_by: None,
        blocking: true,
        state: InboxState::Open,
        question_rev: Some(1),
        answerable: true,
        unanswerable_reason: None,
        attention: None,
        rev: 1,
    }
}

fn plan_row(id: &str) -> PlanRow {
    PlanRow {
        id: id.into(),
        title: "Saved plan".into(),
        role: None,
        linked: None,
        state: PlanState::Executing,
        revision: None,
        delivery: None,
        members_total: 2,
        members_landed: 1,
        blockers: vec![],
        projects: Vec::new(),
        accepted_at: None,
        rev: 1,
    }
}

fn inbox(rows: Vec<InboxRow>, left: Vec<String>, replacement: bool, more: bool) -> Event {
    Event::Rows {
        replacement,
        delta: SubDelta::Inbox {
            delta: RowsDelta {
                epoch: 7,
                rows,
                left,
                cursor: BoardCursor {
                    rev: 1,
                    after: None,
                },
                more,
            },
        },
    }
}

fn plans(rows: Vec<PlanRow>, left: Vec<String>, replacement: bool, more: bool) -> Event {
    Event::Rows {
        replacement,
        delta: SubDelta::Plans {
            delta: RowsDelta {
                epoch: 7,
                rows,
                left,
                cursor: BoardCursor {
                    rev: 1,
                    after: None,
                },
                more,
            },
        },
    }
}

fn restore_device(feed: Signal<Feed>, device: &str) -> Option<((String, String), Cache)> {
    // Exercise the real durable read/project path with an isolated trusted identity source.
    // Native credential file security is tested by credentials.rs, without modifying HOME here.
    wait(Cache::restore_with(feed, |marker| {
        Ok(marker.device == device)
    }))
    .unwrap()
}

#[test]
fn restart_restores_complete_wire_deltas_as_stale_without_fixture_or_readiness() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    reset();
    let feed = live_feed("/workspace/.corral");
    let mut cache = Cache::default();
    wait(cache.authenticated(feed, &session("device-a"))).unwrap();
    wait(cache.capture(&Event::Board {
        epoch: 7,
        rows: vec![board_row("T-1", "removed"), board_row("T-2", "old")],
        left: vec![],
        replacement: true,
    }))
    .unwrap();
    wait(cache.capture(&Event::Board {
        epoch: 7,
        rows: vec![board_row("T-2", "updated"), board_row("T-3", "added")],
        left: vec!["T-1".into()],
        replacement: false,
    }))
    .unwrap();
    wait(cache.capture(&inbox(
        vec![inbox_row("q-1", "removed"), inbox_row("q-2", "old")],
        vec![],
        true,
        false,
    )))
    .unwrap();
    wait(cache.capture(&inbox(
        vec![inbox_row("q-2", "updated question")],
        vec!["q-1".into()],
        false,
        false,
    )))
    .unwrap();
    wait(cache.capture(&plans(vec![plan_row("P-1")], vec![], true, false))).unwrap();
    wait(cache.capture(&plans(
        vec![plan_row("P-2")],
        vec!["P-1".into()],
        false,
        false,
    )))
    .unwrap();
    wait(cache.capture(&Event::Health(Health {
        daemon: DaemonState::Healthy,
        reasons: vec![],
        runtimes: vec![],
        pm: PmState::Live,
        loop_facts: LoopFacts {
            pass_seq: 9,
            ..LoopFacts::default()
        },
        pid: 1,
        version: "test".into(),
    })))
    .unwrap();
    drop(cache);

    let restarted = signal(Feed::default());
    let (namespace, _) = restore_device(restarted, "device-a").unwrap();
    assert_eq!(namespace, ("/workspace/.corral".into(), "device-a".into()));
    restarted.with_untracked(|feed| {
        assert_eq!(feed.link, Link::Down);
        assert!(!feed.ready());
        assert!(!feed.transport_ready);
        assert!(!feed.projection_ready);
        assert!(feed.session.is_some());
        assert_eq!(feed.store.as_deref(), Some("/workspace/.corral"));
        assert_eq!(feed.workspace.as_deref(), Some("/workspace"));
        assert_eq!(feed.synced.as_deref(), Some("10:01"));
        assert!(matches!(&feed.device, DeviceLink::Enrolled { id, .. } if id == "device-a"));
        assert!(feed.capable("inbox.v1"));
        assert!(feed.board.complete);
        assert_eq!(feed.board.tasks().len(), 2);
        assert!(feed.board.task("T-1").is_none());
        assert_eq!(feed.board.task("T-2").unwrap().title, "updated");
        assert_eq!(feed.board.task("T-3").unwrap().title, "added");
        assert_eq!(feed.inbox.len(), 1);
        assert_eq!(feed.inbox.get("q-2").unwrap().title, "updated question");
        assert_eq!(feed.plans.len(), 1);
        assert!(feed.plans.contains_key("P-2"));
        assert_eq!(feed.health.as_ref().unwrap().pass_seq, 9);
        for topic in ["board", "inbox", "plans"] {
            let state = &feed.topics[topic];
            assert!(state.stale);
            assert!(!state.loading);
        }
        assert!(feed.agents.is_empty());
        assert!(feed.threads.is_empty());
    });
    let wrong_device = signal(Feed::default());
    assert!(restore_device(wrong_device, "different-device").is_none());
    wrong_device.with_untracked(|feed| {
        assert!(feed.store.is_none());
        assert!(feed.session.is_none());
        assert!(!feed.board.complete);
    });
}

#[test]
fn partial_or_oversized_updates_never_replace_the_complete_saved_topic() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    reset();
    let mut cache = Cache::default();
    wait(cache.authenticated(live_feed("/workspace/.corral"), &session("device-a"))).unwrap();
    let before = wait(durable::read()).unwrap();
    // A complete incremental page is not a complete topic when its baseline is absent.
    wait(cache.capture(&inbox(
        vec![inbox_row("q-1", "not a snapshot")],
        vec![],
        false,
        false,
    )))
    .unwrap();
    wait(cache.capture(&plans(vec![plan_row("P-partial")], vec![], true, true))).unwrap();
    assert_eq!(wait(durable::read()).unwrap(), before);

    wait(cache.capture(&Event::Board {
        epoch: 7,
        rows: vec![board_row("T-1", "retained")],
        left: vec![],
        replacement: true,
    }))
    .unwrap();
    wait(cache.capture(&inbox(
        vec![inbox_row("q-1", "retained")],
        vec![],
        true,
        false,
    )))
    .unwrap();
    let before = wait(durable::read()).unwrap();
    wait(cache.capture(&inbox(
        vec![inbox_row("q-partial", "partial")],
        vec![],
        true,
        true,
    )))
    .unwrap();
    wait(cache.capture(&Event::Board {
        epoch: 8,
        rows: vec![board_row("T-unknown", "no baseline")],
        left: vec![],
        replacement: false,
    }))
    .unwrap();
    assert_eq!(wait(durable::read()).unwrap(), before);
    let oversized = Event::Board {
        epoch: 7,
        rows: vec![board_row("T-huge", &"x".repeat(MAX_BYTES))],
        left: vec![],
        replacement: true,
    };
    let error = wait(cache.capture(&oversized)).unwrap_err();
    assert!(error.contains("2 MiB"), "{error}");
    assert_eq!(wait(durable::read()).unwrap(), before);
    let restarted = signal(Feed::default());
    restore_device(restarted, "device-a").unwrap();
    restarted.with_untracked(|feed| {
        assert_eq!(feed.board.tasks().len(), 1);
        assert_eq!(feed.board.task("T-1").unwrap().title, "retained");
        assert_eq!(feed.inbox.len(), 1);
        assert!(feed.inbox.contains_key("q-1"));
        assert!(feed.plans.is_empty());
    });
}

#[test]
fn namespace_switch_keeps_pending_words_and_fresh_forget_deletes_only_active_records() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    reset();
    let first = "/first/.corral";
    let second = "/second/.corral";
    let first_namespace = namespace_key(first, "device-a");
    let second_namespace = namespace_key(second, "device-b");
    let mut old_cache = Cache::default();
    wait(old_cache.authenticated(live_feed(first), &session("device-a"))).unwrap();
    wait(old_cache.capture(&Event::Board {
        epoch: 7,
        rows: vec![board_row("T-first", "first")],
        left: vec![],
        replacement: true,
    }))
    .unwrap();
    wait(durable::transaction(|records| {
        records.insert(
            format!("{first_namespace}draft/owner"),
            json!("unsent first words"),
        );
        records.insert(
            format!("{first_namespace}message/queued"),
            json!({"text": "queued first words"}),
        );
        records.insert("unrelated".into(), json!("retained"));
        Ok(())
    }))
    .unwrap();
    let mut new_cache = Cache::default();
    wait(new_cache.authenticated(live_feed(second), &session("device-b"))).unwrap();
    let records = wait(durable::read()).unwrap().1;
    assert!(!records.contains_key(&snapshot_key(&first_namespace)));
    assert!(records.contains_key(&format!("{first_namespace}draft/owner")));
    assert!(records.contains_key(&format!("{first_namespace}message/queued")));
    assert!(wait(old_cache.capture(&Event::Health(Health::down()))).is_err());
    wait(new_cache.capture(&Event::Board {
        epoch: 7,
        rows: vec![board_row("T-second", "second")],
        left: vec![],
        replacement: true,
    }))
    .unwrap();
    wait(durable::transaction(|records| {
        records.insert(
            format!("{second_namespace}draft/owner"),
            json!("explicitly forgotten"),
        );
        records.insert(
            format!("{second_namespace}message/queued"),
            json!({"text": "explicitly forgotten"}),
        );
        Ok(())
    }))
    .unwrap();
    drop(new_cache);
    // No live AppState or Cache context is required to deliberately forget this device.
    wait(Cache::forget()).unwrap();
    let records = wait(durable::read()).unwrap().1;
    assert!(!records.contains_key(ACTIVE));
    assert!(!records.contains_key(&snapshot_key(&second_namespace)));
    assert!(!records.keys().any(|key| key.starts_with(&second_namespace)));
    assert_eq!(
        records[&format!("{first_namespace}draft/owner")],
        "unsent first words"
    );
    assert!(records.contains_key(&format!("{first_namespace}message/queued")));
    assert_eq!(records["unrelated"], "retained");
    assert!(
        wait(Cache::restore(signal(Feed::default()), None))
            .unwrap()
            .is_none()
    );
}
