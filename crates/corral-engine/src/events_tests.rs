use std::time::Duration;

use serde_json::json;

use crate::{
    events::{EventFilter, EventKind, NewEvent},
    model::Actor,
    store::Store,
    tests::task,
};

#[test]
fn event_order_atomicity_and_cross_connection_wait() {
    let root = crate::tests::tempdir();
    let path = root.path().join("corral.db");
    let store = Store::open(&path).unwrap();
    let task = task("T-001");
    store.insert_task(&task).unwrap();
    let new = NewEvent {
        task: Some(task.id.clone()),
        run: None,
        kind: EventKind::Created,
        actor: Actor::Owner,
        detail: json!({"value":1}),
    };
    assert!(store.append_event(&new).is_err());
    let first = store.transaction(|store| store.append_event(&new)).unwrap();
    assert!(
        store
            .transaction::<()>(|store| {
                store.append_event(&new)?;
                anyhow::bail!("abort")
            })
            .is_err()
    );
    assert_eq!(
        store.list_events(&task.id).unwrap(),
        std::slice::from_ref(&first)
    );
    let writer = Store::open(&path).unwrap();
    let append = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        writer
            .transaction(|store| {
                store.append_event(&NewEvent {
                    kind: EventKind::Edited,
                    ..new
                })
            })
            .unwrap()
    });
    let filter = EventFilter {
        kinds: vec![EventKind::Edited],
        task: Some(task.id.clone()),
        project: Some("project".to_owned()),
    };
    let next = store
        .wait(&filter, first.seq, Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert_eq!(next, append.join().unwrap());
    assert!(next.seq > first.seq);
    assert_eq!(store.list_events(&task.id).unwrap(), [first, next.clone()]);
    assert!(
        store
            .wait(&filter, next.seq, Duration::ZERO)
            .unwrap()
            .is_none()
    );
}
