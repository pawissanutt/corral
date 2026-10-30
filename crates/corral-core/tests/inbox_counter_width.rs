//! Physical scalar width is independent of retired inbox positions; full/export shapes stay.
#[path = "store_support/mod.rs"]
mod data;
use corral_core::{
    backend::{TaskNum, TaskPart, TaskParts},
    work::{self, Counter, HotScope},
};

#[test]
fn inbox_counter_digit_boundaries_preserve_full_values_and_exact_head_work() {
    let mut costs = Vec::new();
    for n in [9, 10, 99, 100, 999, 1000] {
        let (_dir, store) = data::store();
        store.fixture_pin_now(data::at(100_000)).unwrap();
        let mut task = data::rich(&store, 1, 1, 1, 1);
        let run = task.runs.last_mut().unwrap();
        let run_id = run.id.clone();
        run.inbox.next_seq = n;
        run.inbox.next_envelope = n;
        let expected = task.clone();
        store.fixture_put_task(&task).unwrap();
        assert_eq!(store.get(&task.id).unwrap(), expected);
        let doc = store
            .view()
            .unwrap()
            .task(TaskNum(1), &TaskParts::All)
            .unwrap()
            .unwrap();
        let row = doc
            .rows
            .iter()
            .find(|row| row.part == TaskPart::Run && row.current)
            .unwrap();
        for key in ["next_seq", "next_envelope"] {
            assert_eq!(row.doc["inbox"][key], format!("{n:020}"));
            assert_eq!(
                serde_json::to_value(&expected).unwrap()["runs"][0]["inbox"][key],
                n
            );
        }
        let hot = HotScope::enter();
        let before = work::snapshot();
        let head = store.head(&task.id).unwrap();
        let run = head.run_head(&run_id).unwrap().unwrap();
        assert_eq!(run.inbox.next_seq, n);
        assert_eq!(run.inbox.next_envelope, n);
        let delta = work::snapshot().since(&before);
        drop(hot);
        delta.check_identity().unwrap();
        assert_eq!(delta.unattributed_hydrations(), 0);
        costs.push(Counter::ALL.map(|counter| delta.get(counter)));
        // Both a full write and a head write preserve the numeric public contract.
        store
            .update_head(&task.id, |head| {
                head.current_run_mut().unwrap().inbox.next_seq = n + 1;
                head.current_run_mut().unwrap().inbox.next_envelope = n + 1;
                Ok(())
            })
            .unwrap();
        let full = serde_json::to_value(store.get(&task.id).unwrap()).unwrap();
        assert_eq!(full["runs"][0]["inbox"]["next_seq"], n + 1);
        assert_eq!(full["runs"][0]["inbox"]["next_envelope"], n + 1);
    }
    for cost in &costs[1..] {
        assert_eq!(
            cost, &costs[0],
            "every cell including actual canonical document bytes"
        );
    }
}

#[test]
fn legacy_numeric_and_new_fixed_width_inbox_counters_decode_identically() {
    let numeric = serde_json::json!({"next_seq":1000,"next_envelope":20});
    let fixed = serde_json::json!({"next_seq":"00000000000000001000","next_envelope":"00000000000000000020"});
    let a: corral_core::inbox::InboxState = serde_json::from_value(numeric).unwrap();
    let b: corral_core::inbox::InboxState = serde_json::from_value(fixed).unwrap();
    assert_eq!(a, b);
    assert_eq!(serde_json::to_value(b).unwrap()["next_seq"], 1000);
}
