//! Task-held PM debt is filtered by canonical row tokens before decode, even on terminal tasks.
mod store_support;
use corral_core::{
    HistorySummary, Status, TaskRef,
    agent::{ReceiptState, RequestReceipt},
    work::{self, Counter, HotScope},
};
use serde_json::json;

fn receipt(id: &str, state: &str, pending: bool) -> RequestReceipt {
    serde_json::from_value(json!({"version":1, "request_id":id, "op":"comment", "payload_sha256":"hash",
        "identity":{"store":"fixture", "project":"app", "role":"pm", "session_id":"session", "generation":1},
        "state":state, "at":store_support::at(0), "result":{"ok":true},
        "post_pending":if pending { vec!["event:comment"] } else { vec![] }})).unwrap()
}

#[test]
fn pending_pm_receipt_selection_and_terminal_owner_retirement_have_exact_growth_cost() {
    let mut costs = Vec::new();
    for scale in [1, 10] {
        let (_dir, store) = store_support::store();
        store.fixture_pin_now(store_support::at(100_000)).unwrap();
        let mut task = store_support::rich(
            &store,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale,
        );
        task.status = Status::Done;
        for receipt in &mut task.agent_pm_receipts {
            receipt.state = ReceiptState::Committed;
            receipt.post_pending.clear();
        }
        for i in 0..2 * scale {
            task.agent_pm_receipts
                .push(receipt(&format!("old-{i:04}"), "committed", false));
        }
        let wanted = [
            receipt("reserved", "reserved", false),
            receipt("queued", "queued", false),
            receipt("uncertain", "uncertain", false),
            receipt("committed-owed", "committed", true),
        ];
        task.agent_pm_receipts.extend(wanted.clone());
        store.save(&task).unwrap();
        let full = store.get(&task.id).unwrap();
        assert_eq!(TaskRef::pending_pm_receipts(&full).unwrap(), wanted);
        let before = work::snapshot();
        let hot = HotScope::enter();
        let owners = store.unfinished_native_heads().unwrap();
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].id, task.id);
        assert_eq!(owners[0].pending_pm_receipts().unwrap(), wanted);
        let head = store
            .update_head(&task.id, |head| {
                for receipt in &wanted {
                    assert!(head.edit_pm_receipt(&receipt.request_id, |stored| {
                        stored.state = ReceiptState::Committed;
                        stored.post_pending.clear();
                        Ok(())
                    })?);
                }
                assert!(head.pending_pm_receipts()?.is_empty());
                Ok(())
            })
            .unwrap();
        assert!(store.unfinished_native_heads().unwrap().is_empty());
        let delta = work::snapshot().since(&before);
        drop(hot);
        delta.check_identity().unwrap();
        assert_eq!(delta.cells(Counter::FullTaskHydrations).total, 0);
        costs.push(Counter::ALL.map(|counter| delta.cells(counter).unattributed));
        let after = store.get(&task.id).unwrap();
        assert_eq!(
            head.summary,
            HistorySummary::of_record(&store.task_record(&task.id).unwrap()).unwrap()
        );
        assert_eq!(after.agent_pm_receipts.len(), full.agent_pm_receipts.len());
        for (before, after) in full.agent_pm_receipts.iter().zip(&after.agent_pm_receipts) {
            if !wanted
                .iter()
                .any(|receipt| receipt.request_id == before.request_id)
            {
                assert_eq!(after, before);
            }
        }
    }
    assert_eq!(
        costs[0], costs[1],
        "every query/mutation/global discovery counter is history-independent"
    );
}
