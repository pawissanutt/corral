//! Retained completion mutators must maintain keyed native rows and bounded owner indexes.
mod store_support;
use corral_core::agent::{self, AgentRunState, ContainerRef, NativeRecord, RequestReceipt};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{AgentHostHead, Status, Store, TaskMut, TaskRef};
use serde_json::json;
use store_support::at;

fn state() -> AgentRunState {
    AgentRunState::new(
        serde_json::from_value(json!({
            "role":"worker", "store":"st-fixture", "project":"app", "generation":1
        }))
        .unwrap(),
    )
}
fn receipt(task: &str, id: &str) -> RequestReceipt {
    serde_json::from_value(json!({
        "version":1,"request_id":id,"op":"done","payload_sha256":"hash",
        "identity":{"store":"st-fixture","project":"app","role":"worker","task":task,
            "run":"R0001","session_id":"session","generation":1},
        "state":"committed","at":at(0)
    }))
    .unwrap()
}
fn records(task: &str, n: u32) -> [NativeRecord; 10] {
    let mut receipt = receipt(task, &format!("receipt-{n}"));
    receipt.post_pending.push("effect:event".into());
    [
        NativeRecord::Generation(Box::new(serde_json::from_value(json!({
            "n":n,"bind_nonce_sha256":"00","pane":"p1","created":at(0)
        })).unwrap())),
        NativeRecord::Receipt(receipt),
        NativeRecord::Question(serde_json::from_value(json!({
            "version":1,"key":format!("{task}/R0001/Q{n}"),"revision":1,
            "asked":{"request_id":format!("q-{n}"),"payload_sha256":"hash",
                "session_id":"session","generation":1,"at":at(0)},
            "kind":"clarification","blocking":false,"text":"Which?","state":"open",
            "wake":{"route":"none","key":format!("qw-{n}"),"state":"pending"}
        })).unwrap()),
        NativeRecord::Turn(serde_json::from_value(json!({
            "generation":1,"seq":n,"origin":{"kind":"bridge"},"started":at(0),"ended":at(1)
        })).unwrap()),
        NativeRecord::Wake(serde_json::from_value(json!({
            "key":format!("wake-{n}"),"request_id":format!("wake-{n}"),
            "kind":"wait","target":"W1","state":"armed","at":at(0)
        })).unwrap()),
        NativeRecord::Report(serde_json::from_value(json!({
            "version":1,"kind":"done","request_id":format!("report-{n}"),"generation":1,"at":at(0)
        })).unwrap()),
        NativeRecord::Job(serde_json::from_value(json!({
            "id":format!("job-{n}"),"generation":1,"command":"true",
            "state":"prepared","prepared_at":at(0)
        })).unwrap()),
        NativeRecord::Rewind(serde_json::from_value(json!({
            "id":format!("rewind-{n}"),"checkpoint":"g1-t1","commit":"commit","head":"head",
            "leaf":"leaf","from_generation":1,"stage":"aborted","at":at(0),"updated":at(1)
        })).unwrap()),
        NativeRecord::UsageSession(serde_json::from_value(json!({
            "generation":1,"kind":"oracle","session_id":format!("usage-{n}"),
            "session_file":"/fixture/usage.jsonl","model":"fixture","request":format!("usage-{n}")
        })).unwrap()),
        NativeRecord::BudgetRequest(serde_json::from_value(json!({
            "request":format!("budget-{n}"),"generation":1,"tokens":100,"usd_nanos":1000,
            "priced":true,"response":format!("response-{n}")
        })).unwrap()),
    ]
}

#[test]
fn replacement_rebuilds_all_ten_positions_and_pending_before_same_worker_appends() {
    for replace in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        let mut expected = store_support::rich(&store, 1, 0, 0, 0);
        let id = expected.id.clone();
        let owner = ContainerRef::Worker {
            task: id.clone(),
            run: expected.runs[0].id.clone(),
        };
        expected
            .init_agent_head(&owner, AgentHostHead::of(&state()))
            .unwrap();
        for record in records(&id, 1) {
            expected.put_agent_record(&owner, record).unwrap();
        }
        store.fixture_put_task(&expected).unwrap();
        if replace {
            for record in records(&id, 2) {
                expected.put_agent_record(&owner, record).unwrap();
            }
        }
        let replacement = agent::container_state(&expected, &owner).unwrap().clone();
        store
            .update_head(&id, |head| {
                head.set_current_agent_state(Box::new(replacement))
            })
            .unwrap();
        let count = if replace { 2 } else { 1 };
        let projected = store
            .head(&id)
            .unwrap()
            .agent_head(&owner)
            .unwrap()
            .unwrap();
        assert_eq!(projected.records.counts, [count; 10]);
        assert_eq!(projected.records.pending, 3 * count);
        drop(store);
        let reopened = Store::open(root.path()).unwrap();
        for record in records(&id, 3) {
            expected.put_agent_record(&owner, record.clone()).unwrap();
            reopened
                .update_head(&id, |head| head.put_agent_record(&owner, record).map(drop))
                .unwrap();
        }
        let full = reopened.get(&id).unwrap();
        assert_eq!(
            agent::container_state(&full, &owner),
            agent::container_state(&expected, &owner)
        );
        let projected = reopened
            .head(&id)
            .unwrap()
            .agent_head(&owner)
            .unwrap()
            .unwrap();
        assert_eq!(projected.records.counts, [count + 1; 10]);
        assert_eq!(projected.records.pending, 3 * (count + 1));
        assert_eq!(
            projected,
            AgentHostHead::of(agent::container_state(&expected, &owner).unwrap())
        );
    }
}

#[test]
fn receipt_editor_drains_current_and_historical_owners_with_equal_work_under_growth() {
    for historical in [false, true] {
        let mut budgets = Vec::new();
        for scale in [1, 10] {
            let root = tempfile::tempdir().unwrap();
            let store = Store::init(root.path()).unwrap();
            store.fixture_pin_now(at(0)).unwrap();
            let mut task = store.create("Terminal", None, Status::Done, "").unwrap();
            task.runs = vec![store_support::coding_run(&task.id, "R0001", 0, 0, 0, true)];
            task.runs[0].inbox.settled = Some(at(1));
            let mut host = state();
            host.receipts = (0..2 * scale)
                .map(|n| receipt(&task.id, &format!("old-{n:04}")))
                .collect();
            let mut target = receipt(&task.id, "target");
            target.post_pending.push("effect:event".into());
            host.receipts.push(target.clone());
            task.runs[0].agent_host = Some(Box::new(host));
            if historical {
                let mut next = store_support::coding_run(&task.id, "R0002", 2, 0, 0, true);
                next.inbox.settled = Some(at(3));
                task.runs.push(next);
            }
            store.fixture_put_task(&task).unwrap();
            let owner = ContainerRef::Worker {
                task: task.id.clone(),
                run: "R0001".into(),
            };
            assert_eq!(store.unfinished_native_heads().unwrap().len(), 1);
            assert_eq!(
                store
                    .head(&task.id)
                    .unwrap()
                    .unfinished_native_containers()
                    .unwrap(),
                vec![owner.clone()]
            );
            let delta;
            {
                let _hot = HotScope::enter();
                let before = work::snapshot();
                store
                    .update_head(&task.id, |head| {
                        head.edit_worker_receipt("R0001", "target", |r| {
                            r.post_pending.clear();
                            Ok(())
                        })
                    })
                    .unwrap();
                let head = store.head(&task.id).unwrap();
                assert!(head.pending_agent_receipts(&owner).unwrap().is_empty());
                assert_eq!(head.agent_head(&owner).unwrap().unwrap().records.pending, 0);
                assert!(head.unfinished_native_containers().unwrap().is_empty());
                assert!(store.unfinished_native_heads().unwrap().is_empty());
                delta = work::snapshot().since(&before);
            }
            delta.check_identity().unwrap();
            assert_eq!(delta.total(Counter::FullTaskHydrations), 0);
            budgets.push(delta);
            target.post_pending.clear();
            task.runs[0]
                .agent_host
                .as_mut()
                .unwrap()
                .receipts
                .last_mut()
                .unwrap()
                .clone_from(&target);
            let full = store.get(&task.id).unwrap();
            assert_eq!(full.runs[0].agent_host, task.runs[0].agent_host);
            let before = full.runs[0].agent_host.as_ref().unwrap().receipts.clone();
            // A no-op edit cannot decrement twice; re-arming must restore the pending owner.
            store
                .update_head(&task.id, |head| {
                    head.edit_worker_receipt("R0001", "target", |_| Ok(()))
                })
                .unwrap();
            store
                .update_head(&task.id, |head| {
                    head.edit_worker_receipt("R0001", "target", |r| {
                        r.post_pending.push("effect:event".into());
                        Ok(())
                    })
                })
                .unwrap();
            let head = store.head(&task.id).unwrap();
            assert_eq!(head.agent_head(&owner).unwrap().unwrap().records.pending, 1);
            assert_eq!(head.pending_agent_receipts(&owner).unwrap().len(), 1);
            assert_eq!(head.unfinished_native_containers().unwrap(), vec![owner]);
            assert_eq!(store.unfinished_native_heads().unwrap().len(), 1);
            assert_eq!(
                &store.get(&task.id).unwrap().runs[0]
                    .agent_host
                    .as_ref()
                    .unwrap()
                    .receipts[..2 * scale],
                &before[..2 * scale]
            );
        }
        assert_eq!(budgets[0], budgets[1]);
    }
}
