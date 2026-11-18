use super::super::durable::wait;
use super::*;
use corral_protocol::{DaemonState, subscription::BoardCursor, subscription::RowsDelta};
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
        "seen": {"through": 0}, "server_time": "2026-11-15T10:00:00Z"
    }))
    .unwrap()
}

fn live_feed(store: &str) -> Signal<Feed> {
    signal(Feed {
        store: Some(store.into()),
        clock: "10:01".into(),
        synced: Some("10:00".into()),
        capabilities: BTreeSet::from(["tasks.v1".into(), "plans.v1".into()]),
        ..Feed::default()
    })
}

fn task(id: &str, title: &str) -> corral_types::Task {
    crate::model::daemon::bare_task(
        id,
        title,
        corral_types::Status::Ready,
        jiff::Timestamp::UNIX_EPOCH,
    )
}

fn delta<T>(epoch: u64, rows: Vec<T>, left: Vec<String>, more: bool) -> RowsDelta<T> {
    RowsDelta {
        epoch,
        rows,
        left,
        cursor: BoardCursor {
            rev: 1,
            after: None,
        },
        more,
    }
}

fn tasks(epoch: u64, rows: Vec<corral_types::Task>, left: Vec<String>, replacement: bool) -> Event {
    Event::Rows {
        replacement,
        delta: SubDelta::Tasks {
            delta: delta(
                epoch,
                rows.into_iter()
                    .map(crate::model::daemon::task_row)
                    .collect(),
                left,
                false,
            ),
        },
    }
}

fn plans(ids: &[&str], left: Vec<String>, replacement: bool, more: bool) -> Event {
    let rows = ids
        .iter()
        .map(|id| PlanRow {
            parent: task(id, "Saved plan"),
            child_counts: Vec::new(),
        })
        .collect();
    Event::Rows {
        replacement,
        delta: SubDelta::Plans {
            delta: delta(7, rows, left, more),
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
    wait(cache.capture(&tasks(
        7,
        vec![task("T-1", "removed"), task("T-2", "old")],
        vec![],
        true,
    )))
    .unwrap();
    wait(cache.capture(&tasks(
        7,
        vec![task("T-2", "updated"), task("T-3", "added")],
        vec!["T-1".into()],
        false,
    )))
    .unwrap();
    wait(cache.capture(&plans(&["T-8"], vec![], true, false))).unwrap();
    wait(cache.capture(&plans(&["T-9"], vec!["T-8".into()], false, false))).unwrap();
    wait(cache.capture(&Event::Health(Health {
        daemon: DaemonState::Healthy,
        runner_reachable: true,
        last_sync_at: None,
        last_sync_error: None,
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
        assert!(feed.capable("plans.v1"));
        assert!(feed.board.complete);
        assert_eq!(feed.board.tasks().len(), 2);
        assert!(feed.board.task("T-1").is_none());
        assert_eq!(feed.board.task("T-2").unwrap().title(), "updated");
        assert_eq!(feed.board.task("T-3").unwrap().title(), "added");
        assert_eq!(feed.plans.len(), 1);
        assert!(feed.plans.contains_key("T-9"));
        assert!(feed.health.as_ref().unwrap().runner_reachable);
        for topic in ["tasks", "plans"] {
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
    wait(cache.capture(&plans(&["T-8"], vec![], false, false))).unwrap();
    wait(cache.capture(&plans(&["T-partial"], vec![], true, true))).unwrap();
    assert_eq!(wait(durable::read()).unwrap(), before);

    wait(cache.capture(&tasks(7, vec![task("T-1", "retained")], vec![], true))).unwrap();
    let before = wait(durable::read()).unwrap();
    wait(cache.capture(&tasks(
        8,
        vec![task("T-unknown", "no baseline")],
        vec![],
        false,
    )))
    .unwrap();
    assert_eq!(wait(durable::read()).unwrap(), before);
    let oversized = tasks(
        7,
        vec![task("T-huge", &"x".repeat(MAX_BYTES))],
        vec![],
        true,
    );
    let error = wait(cache.capture(&oversized)).unwrap_err();
    assert!(error.contains("2 MiB"), "{error}");
    assert_eq!(wait(durable::read()).unwrap(), before);
    let restarted = signal(Feed::default());
    restore_device(restarted, "device-a").unwrap();
    restarted.with_untracked(|feed| {
        assert_eq!(feed.board.tasks().len(), 1);
        assert_eq!(feed.board.task("T-1").unwrap().title(), "retained");
        assert!(feed.plans.is_empty());
    });
}

#[test]
fn a_snapshot_saved_by_the_old_wire_is_dropped_not_misread() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    reset();
    let mut cache = Cache::default();
    wait(cache.authenticated(live_feed("/workspace/.corral"), &session("device-a"))).unwrap();
    let namespace = namespace_key("/workspace/.corral", "device-a");
    wait(durable::transaction(|records| {
        records.insert(
            snapshot_key(&namespace),
            json!({"version": 1, "board": {"epoch": 1, "rows": {"T-1": {"id": "T-1"}}}}),
        );
        Ok(())
    }))
    .unwrap();
    drop(cache);
    let restarted = signal(Feed::default());
    restore_device(restarted, "device-a").unwrap();
    restarted.with_untracked(|feed| {
        assert!(feed.session.is_some());
        assert!(!feed.board.complete);
        assert!(feed.board.tasks().is_empty());
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
    wait(old_cache.capture(&tasks(7, vec![task("T-first", "first")], vec![], true))).unwrap();
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
    wait(new_cache.capture(&tasks(7, vec![task("T-second", "second")], vec![], true))).unwrap();
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
