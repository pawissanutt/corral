//! Worker-only indexes exclude child/planning aliases before decoding retained native history.
mod store_support;
use corral_core::TaskRef;
use corral_core::agent::{
    AgentRunState, NativeRecord, RequestReceipt, WakeState, WakeSubscription,
};
use corral_core::work::{self, Counter, HotScope};
use serde_json::json;

fn host(role: &str, n: usize) -> Box<AgentRunState> {
    let mut state = AgentRunState::new(
        serde_json::from_value(
            json!({"role":role,"store":"fixture","project":"app","generation":1}),
        )
        .unwrap(),
    );
    for i in 0..n {
        state
            .receipts
            .push(receipt(role, &format!("old-{i:04}"), "old", "old"));
        state.wakes.push(wake(&format!("old-{i:04}"), "fired"));
    }
    Box::new(state)
}
fn receipt(role: &str, id: &str, reference: &str, reason: &str) -> RequestReceipt {
    serde_json::from_value(json!({"version":1,"request_id":id,"op":"gate","payload_sha256":"hash",
        "identity":{"store":"fixture","project":"app","role":role,"task":"T-001","run":"R0001","session_id":"session","generation":1},
        "state":"committed","at":store_support::at(0),"effect":{"kind":"gate_attempt","ref":reference},"result":{"queued":true,"reason":reason}})).unwrap()
}
fn wake(id: &str, state: &str) -> WakeSubscription {
    serde_json::from_value(json!({"key":id,"request_id":id,"kind":"wait","target":"W0999","state":state,"at":store_support::at(0)})).unwrap()
}

#[test]
fn active_worker_wakes_and_first_queued_effect_are_scoped_keyed_and_history_independent() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let (_dir, store) = store_support::store();
        let mut task = store_support::rich(
            &store,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale,
        );
        for run in &mut task.runs {
            run.agent_host = Some(host("worker", 2 * scale));
        }
        // This matching child precedes the real worker's receipt in storage order.
        let child = task.runs[0].review_runs.first_mut().unwrap();
        let mut other = host("reviewer", 2 * scale);
        other
            .receipts
            .push(receipt("reviewer", "alias", "attempt", "wrong reviewer"));
        for i in 0..2 * scale {
            other.wakes.push(wake(&format!("ignored-{i:04}"), "armed"));
        }
        child.agent_host = Some(other);
        task.plan_runs[0].agent_host = Some(host("planner", 2 * scale));
        task.plan_runs[0]
            .agent_host
            .as_mut()
            .unwrap()
            .receipts
            .push(receipt("planner", "alias-plan", "attempt", "wrong planner"));
        let current = task.runs.last_mut().unwrap();
        let container = corral_core::agent::ContainerRef::Worker {
            task: task.id.clone(),
            run: current.id.clone(),
        };
        let state = current.agent_host.as_mut().unwrap();
        state
            .receipts
            .push(receipt("worker", "worker-first", "attempt", "first worker"));
        state
            .receipts
            .push(receipt("worker", "worker-later", "attempt", "later worker"));
        state.wakes.push(wake("active", "armed"));
        state.wakes.push(wake("pending-start", "pending_start"));
        let expected = task.active_worker_wakes().unwrap();
        assert_eq!(expected.len(), 2);
        store.fixture_put_task(&task).unwrap();
        let pinned = store.head(&task.id).unwrap();
        let hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(pinned.active_worker_wakes().unwrap(), expected);
        let selected = pinned.worker_queued_effect_receipt("attempt").unwrap();
        assert_eq!(
            selected,
            task.worker_queued_effect_receipt("attempt").unwrap()
        );
        assert_eq!(selected.unwrap().request_id, "worker-first");
        assert!(
            pinned
                .worker_queued_effect_receipt("absent")
                .unwrap()
                .is_none()
        );
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|c| after.total(c) - before.total(c)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
        drop(hot);
        store
            .update_head(&task.id, |head| {
                let mut record = head.agent_wake(&container, "active")?.unwrap();
                record.state = WakeState::Fired;
                head.put_agent_record(&container, NativeRecord::Wake(record))?;
                let mut receipt = head.agent_receipt(&container, "worker-first")?.unwrap();
                receipt.result["queued"] = json!(false);
                head.put_agent_record(&container, NativeRecord::Receipt(receipt))?;
                assert_eq!(TaskRef::active_worker_wakes(head)?.len(), 1);
                assert_eq!(
                    TaskRef::worker_queued_effect_receipt(head, "attempt")?
                        .unwrap()
                        .request_id,
                    "worker-later"
                );
                Ok(())
            })
            .unwrap();
        assert_eq!(pinned.active_worker_wakes().unwrap(), expected);
        let fresh = store.head(&task.id).unwrap();
        let full = store.get(&task.id).unwrap();
        assert_eq!(
            fresh.active_worker_wakes().unwrap(),
            full.active_worker_wakes().unwrap()
        );
        assert_eq!(
            fresh.worker_queued_effect_receipt("attempt").unwrap(),
            full.worker_queued_effect_receipt("attempt").unwrap()
        );
    }
    assert_eq!(measured[0], measured[1]);
}
