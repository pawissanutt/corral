use corral_core::{Actor, Event, Store};
use serde_json::{Value, json};

#[test]
fn legacy_events_round_trip_without_attribution_fields() {
    // Given old log lines, including missing task and legacy detail attribution.
    let fixture = include_str!("fixtures/legacy-events.jsonl");
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    std::fs::write(store.dir().join("events.jsonl"), fixture).unwrap();
    // When the Store reads every legacy line and each event is serialized again.
    let events = store.events().unwrap();
    assert_eq!(events.len(), fixture.lines().count());
    for (event, line) in events.iter().zip(fixture.lines()) {
        let serialized = serde_json::to_value(event).unwrap();
        // Then none are lost or backfilled, and the old JSON data is preserved.
        assert_eq!(event.actor, None);
        assert_eq!(event.via, None);
        assert!(serialized.get("actor").is_none());
        assert!(serialized.get("via").is_none());
        assert_eq!(serialized, serde_json::from_str::<Value>(line).unwrap());
        assert_eq!(serde_json::from_value::<Event>(serialized).unwrap(), *event);
    }
}

#[test]
fn all_actor_variants_serialize_as_the_approved_names() {
    for (actor, name) in [
        (Actor::Owner, "owner"),
        (Actor::Pm, "pm"),
        (Actor::Agent, "agent"),
        (Actor::Autopilot, "autopilot"),
        (Actor::Reviewer, "reviewer"),
        (Actor::System, "system"),
    ] {
        // Given an attributed event for each supported actor.
        let event = Event {
            ts: "2026-10-18T00:00:00Z".parse().unwrap(),
            task: Some("T-001".into()),
            kind: "gate".into(),
            actor: Some(actor),
            via: Some("run:R1".into()),
            detail: json!({}),
        };
        // When it crosses the JSON boundary.
        let serialized = serde_json::to_value(&event).unwrap();
        // Then the actor and via survive with their wire names.
        assert_eq!(serialized["actor"], name);
        assert_eq!(serialized["via"], "run:R1");
        assert_eq!(serde_json::from_value::<Event>(serialized).unwrap(), event);
    }
}

#[test]
fn log_event_leaves_unconfigured_context_unset() {
    // Given a Store with no entry-point defaults.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    // When an ordinary event is logged.
    store.log_event(None, "sync", json!({})).unwrap();
    // Then the stored JSON has no envelope attribution.
    let line = std::fs::read_to_string(store.dir().join("events.jsonl")).unwrap();
    let serialized: Value = serde_json::from_str(&line).unwrap();
    assert!(serialized.get("actor").is_none());
    assert!(serialized.get("via").is_none());
}

#[test]
fn nested_guards_restore_actor_and_preserve_via_on_store_clones() {
    // Given a process default shared by clones.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    store.set_attribution(Actor::Owner, "tui").unwrap();
    let clone = store.clone();
    // When events are written before, inside, and after nested actor scopes.
    store.log_event(None, "before", json!({})).unwrap();
    {
        let _outer = store.attribute(Actor::Autopilot);
        clone.log_event(None, "outer", json!({})).unwrap();
        {
            let _inner = clone.attribute(Actor::Reviewer);
            store.log_event(None, "inner", json!({})).unwrap();
        }
        store.log_event(None, "restored_outer", json!({})).unwrap();
    }
    clone
        .log_event(None, "restored_default", json!({}))
        .unwrap();
    // Then each scope restores its predecessor and leaves the surface unchanged.
    let events = store.events().unwrap();
    assert_eq!(
        events.iter().map(|event| event.actor).collect::<Vec<_>>(),
        [
            Some(Actor::Owner),
            Some(Actor::Autopilot),
            Some(Actor::Reviewer),
            Some(Actor::Autopilot),
            Some(Actor::Owner),
        ]
    );
    assert!(
        events
            .iter()
            .all(|event| event.via.as_deref() == Some("tui"))
    );
}

#[test]
fn explicit_detail_actor_wins_without_rewriting_detail_or_via() {
    // Given both a default and a scoped actor that differ from the explicit author.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    store.set_attribution(Actor::Owner, "cli").unwrap();
    let _guard = store.attribute(Actor::Autopilot);
    let detail = json!({"actor":"pm","from":"idea","to":"ready"});
    // When the explicitly authored event is logged.
    store
        .log_event(Some("T-003"), "moved", detail.clone())
        .unwrap();
    // Then the explicit author wins while the source and legacy detail survive.
    let events = store.events().unwrap();
    assert_eq!(events[0].actor, Some(Actor::Pm));
    assert_eq!(events[0].via.as_deref(), Some("cli"));
    assert_eq!(events[0].detail, detail);
}

#[test]
fn scopes_work_without_defaults_and_remain_store_local() {
    // Given independent Stores, including one opened at the same root.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let independent = Store::open(temp.path()).unwrap();
    // When one handle installs a scope without a default.
    {
        let _guard = store.attribute(Actor::System);
        independent
            .log_event(None, "independent", json!({}))
            .unwrap();
        store.log_event(None, "scoped", json!({})).unwrap();
    }
    store.log_event(None, "restored", json!({})).unwrap();
    // Then no attribution leaks to an independent handle or past the guard.
    let events = store.events().unwrap();
    assert_eq!(events[0].actor, None);
    assert_eq!(events[1].actor, Some(Actor::System));
    assert_eq!(events[2].actor, None);
    assert!(events.iter().all(|event| event.via.is_none()));
}

#[test]
fn batch_publication_keeps_the_invoking_default_and_scope() {
    // Given an entry-point default and an active scope on the invoking Store.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    store.set_attribution(Actor::Owner, "cli").unwrap();
    let _guard = store.attribute(Actor::Autopilot);
    // When a batch is published through its freshly reopened Store.
    store
        .create_batch(1, false, |_, tasks| {
            tasks[0].title = "Batch child".into();
            Ok(Vec::new())
        })
        .unwrap();
    // Then the imported event keeps the scoped actor and the default surface.
    let imported = store
        .events()
        .unwrap()
        .into_iter()
        .find(|event| event.kind == "imported")
        .unwrap();
    assert_eq!(imported.actor, Some(Actor::Autopilot));
    assert_eq!(imported.via.as_deref(), Some("cli"));
}

#[test]
fn worker_scope_does_not_relabel_the_owner_thread() {
    // Given a default shared with a worker and exact signals for its scope.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    store.set_attribution(Actor::Owner, "tui").unwrap();
    let worker = store.clone();
    let (scoped_tx, scoped_rx) = std::sync::mpsc::channel();
    let (logged_tx, logged_rx) = std::sync::mpsc::channel();
    // When the owner logs while the worker's override is active.
    std::thread::scope(|threads| {
        threads.spawn(move || {
            let _guard = worker.attribute(Actor::Autopilot);
            scoped_tx.send(()).unwrap();
            logged_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            worker.log_event(None, "worker", json!({})).unwrap();
        });
        scoped_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        store.log_event(None, "owner", json!({})).unwrap();
        logged_tx.send(()).unwrap();
    });
    // Then each thread records its actual actor, using the same surface.
    let events = store.events().unwrap();
    assert_eq!(events[0].actor, Some(Actor::Owner));
    assert_eq!(events[1].actor, Some(Actor::Autopilot));
    assert!(
        events
            .iter()
            .all(|event| event.via.as_deref() == Some("tui"))
    );
}

#[test]
fn default_can_only_be_set_once_across_clones() {
    // Given an initialized process default.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    store.set_attribution(Actor::Agent, "run:R1").unwrap();
    // When a clone tries to replace it.
    assert!(store.clone().set_attribution(Actor::Owner, "cli").is_err());
    store.log_event(None, "gate", json!({})).unwrap();
    // Then the original default remains authoritative.
    let events = store.events().unwrap();
    assert_eq!(events[0].actor, Some(Actor::Agent));
    assert_eq!(events[0].via.as_deref(), Some("run:R1"));
}

#[test]
fn dropped_outer_guard_does_not_resurrect_after_inner_guard() {
    // Given two scopes that will be dropped out of lexical order.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let outer = store.attribute(Actor::Autopilot);
    let inner = store.attribute(Actor::Reviewer);
    // When the outer guard is dropped before the inner.
    drop(outer);
    store.log_event(None, "inner", json!({})).unwrap();
    drop(inner);
    store.log_event(None, "restored", json!({})).unwrap();
    // Then only live scopes contribute attribution.
    let events = store.events().unwrap();
    assert_eq!(events[0].actor, Some(Actor::Reviewer));
    assert_eq!(events[1].actor, None);
}
