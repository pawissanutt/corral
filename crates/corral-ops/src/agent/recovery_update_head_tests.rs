//! Settled native receipt still owes recovery evidence across a terminal-owner discovery pass.
use super::*;
use crate::agent::native_test_support::{self as native, data};
use corral_core::{
    RunOutcome, Status,
    agent::AgentRole,
    work::{self, Counter, HotScope},
};

#[test]
fn recovery_update_debt_survives_closed_history_and_retires_once_with_fixed_work() {
    let mut costs = Vec::new();
    for scale in [1, 10] {
        corral_core::clock::pin_thread(Some(data::at(100_000)));
        let (_dir, store) = data::store();
        store.fixture_pin_now(data::at(100_000)).unwrap();
        let mut task = data::rich(
            &store,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale,
        );
        task.status = Status::Done;
        let run = task.runs.first_mut().unwrap();
        run.outcome = Some(RunOutcome::Done);
        run.inbox.settled = Some(data::at(20));
        let container = ContainerRef::Worker {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        let mut host = native::host(AgentRole::Worker, 2 * scale);
        let receipt = |id: String, noticed: bool| {
            serde_json::from_value(json!({"version":1,"request_id":id,
            "op":"gate","payload_sha256":"hash", "identity":{"store":"fixture","project":"app",
                "role":"worker","task":task.id,"run":run.id,"session_id":"session","generation":999},
            "state":"committed", "at":data::at(0), "result":{"outcome":"pass"},
            "evidence": if noticed { vec![json!({"state":"in_progress", "source":NOTICE_SOURCE, "at":data::at(0)})] } else {vec![]}
        })).unwrap()
        };
        for i in 0..2 * scale {
            host.receipts.push(receipt(format!("old-{i:04}"), false));
        }
        host.receipts.push(receipt("owed-update".into(), true));
        let before_receipts = host.receipts.clone();
        run.agent_host = Some(Box::new(host));
        store.fixture_put_task(&task).unwrap();
        let ctx = Ctx::from_store(store);
        let before = work::snapshot();
        let hot = HotScope::enter();
        assert_eq!(ctx.store.unfinished_native_heads().unwrap().len(), 1);
        assert_eq!(
            post_updates(&ctx, &task.id).unwrap(),
            0,
            "closed containers publish no message"
        );
        assert!(ctx.store.unfinished_native_heads().unwrap().is_empty());
        assert_eq!(post_updates(&ctx, &task.id).unwrap(), 0);
        let delta = work::snapshot().since(&before);
        drop(hot);
        delta.check_identity().unwrap();
        assert_eq!(delta.cells(Counter::FullTaskHydrations).total, 0);
        costs.push(Counter::ALL.map(|counter| delta.cells(counter).unattributed));
        let actual = ctx.store.get(&task.id).unwrap();
        let actual = agent::container_state(&actual, &container).unwrap();
        assert_eq!(
            &actual.receipts[..actual.receipts.len() - 1],
            &before_receipts[..before_receipts.len() - 1]
        );
        let updated = actual.receipts.last().unwrap();
        assert!(
            updated
                .evidence
                .iter()
                .any(|e| e.source == UPDATE_SOURCE && e.state == "closed")
        );
        assert!(!updated.recovery_update_due());
    }
    assert_eq!(
        costs[0], costs[1],
        "every discovery/update/replay counter is independent of native/task history"
    );
}
