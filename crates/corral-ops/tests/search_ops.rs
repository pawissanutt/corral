use corral_core::query::Query;
use corral_core::{Status, Store};
use corral_ops::{SearchData, search_tasks};
use jiff::Timestamp;
use serde_json::json;

fn now() -> Timestamp {
    "2026-10-15T12:00:00Z".parse().unwrap()
}

#[test]
fn snapshot_search_ranks_all_statuses_and_loads_pin_and_landing_metadata() {
    // Given active, idea, done and dropped tasks in an isolated file store.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let title = store.create("nebula", None, Status::Idea, "").unwrap();
    let body = store
        .create("archived", Some("corral"), Status::Done, "nebula")
        .unwrap();
    let dropped = store
        .create("discarded nebula", None, Status::Dropped, "")
        .unwrap();
    store
        .create("running", None, Status::Running, "nebula")
        .unwrap();
    store.pin(&body.id, None, false).unwrap();
    store
        .log_event(
            Some(&body.id),
            "approved",
            json!({"sha": "abcdef123456", "strategy": "fast_forward"}),
        )
        .unwrap();

    // When searching the snapshot for text and loaded supplemental metadata.
    let data = SearchData::load(&store).unwrap();
    let ranked = data.ranked(&Query::parse("nebula", now()).unwrap());
    let pinned = data.ranked(&Query::parse("pinned sha:abcdef ff", now()).unwrap());
    let discarded = search_tasks(&store, "status:dropped").unwrap();

    // Then title matches rank first, all statuses participate and pin/history data match.
    assert_eq!(ranked.len(), 4);
    assert_eq!(data.tasks[ranked[0]].id, title.id);
    assert_eq!(data.tasks[ranked[1]].id, dropped.id);
    assert_eq!(
        pinned
            .iter()
            .map(|&i| &data.tasks[i].id)
            .collect::<Vec<_>>(),
        [&body.id]
    );
    assert_eq!(discarded.len(), 1);
    assert_eq!(discarded[0].id, dropped.id);
}

#[test]
fn equal_relevance_uses_fixed_update_times_then_task_ids() {
    // Given tied text scores with explicit timestamps, not elapsed wall time.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    for _ in 0..3 {
        store.create("same", None, Status::Backlog, "").unwrap();
    }
    let mut data = SearchData::load(&store).unwrap();
    let old: Timestamp = "2026-10-01T00:00:00Z".parse().unwrap();
    for task in &mut data.tasks {
        task.updated = now();
    }
    data.tasks[2].updated = old;
    let expected = [
        data.tasks[1].id.clone(),
        data.tasks[0].id.clone(),
        data.tasks[2].id.clone(),
    ];

    // When ranking otherwise identical matches.
    let ids: Vec<_> = data
        .ranked(&Query::parse("same", now()).unwrap())
        .into_iter()
        .map(|i| data.tasks[i].id.clone())
        .collect();

    // Then update time wins and higher ids break the remaining tie.
    assert_eq!(ids, expected);
}

#[test]
fn unknown_fields_fail_in_the_shared_search_service() {
    // Given an empty isolated store.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();

    // When a client uses an unknown field.
    let error = search_tasks(&store, "unknown:yes").unwrap_err();

    // Then it gets a query error rather than silently empty results.
    assert!(error.to_string().contains("unknown query field"));
}
