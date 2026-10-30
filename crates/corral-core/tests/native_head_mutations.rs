//! Native record edits preserve full-facade state without hydrating native history.
mod store_support;
use corral_core::agent::{
    self, AgentRunState, ContainerRef, NativeRecord, ReceiptState, RequestReceipt,
};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{AgentHostHead, Store, TaskMut, TaskRef};
use serde_json::json;
use store_support::at;

fn state(role: &str) -> AgentRunState {
    AgentRunState::new(
        serde_json::from_value(json!({
            "role":role,"store":"st-fixture","project":"app","generation":10000
        }))
        .unwrap(),
    )
}
fn receipt(task: &str, id: &str, status: &str) -> RequestReceipt {
    serde_json::from_value(json!({
        "version":1,"request_id":id,"op":"gate","payload_sha256":"hash",
        "identity":{"store":"st-fixture","project":"app","role":"worker","task":task,
            "run":"R0001","session_id":"session","generation":10000},
        "state":status,"at":at(0)
    }))
    .unwrap()
}
fn turn(generation: u32, seq: u64) -> NativeRecord {
    NativeRecord::Turn(serde_json::from_value(json!({
        "generation":generation,"seq":seq,"origin":{"kind":"bridge"},"started":at(0),"ended":at(1)
    })).unwrap())
}

#[test]
fn native_keyed_mutations_match_full_facade_for_all_container_roles() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let mut expected = store_support::rich(&store, 2, 2, 2, 2);
    let id = expected.id.clone();
    let parent = expected.runs.last().unwrap();
    let run = parent.id.clone();
    let child = parent.review_runs.last().unwrap().id.clone();
    let plan = expected.plans.last().unwrap();
    let plan_id = plan.id.clone();
    let review = plan.reviews.last().unwrap().id.clone();
    let planner = expected.plan_runs.last().unwrap().id.clone();
    let cases = [
        (
            ContainerRef::Worker {
                task: id.clone(),
                run: run.clone(),
            },
            "worker",
        ),
        (
            ContainerRef::Reviewer {
                task: id.clone(),
                run,
                review: child,
            },
            "reviewer",
        ),
        (
            ContainerRef::Planner {
                task: id.clone(),
                run: planner,
            },
            "planner",
        ),
        (
            ContainerRef::PlanReviewer {
                task: id.clone(),
                plan: plan_id,
                review,
            },
            "plan_reviewer",
        ),
    ];
    for (container, role) in cases {
        let header = AgentHostHead::of(&state(role));
        expected
            .init_agent_head(&container, header.clone())
            .unwrap();
        store
            .update_head(&id, |head| {
                TaskMut::init_agent_head(head, &container, header)
            })
            .unwrap();
        let mut records = vec![
            turn(10000, 1),
            turn(10000, 2),
            NativeRecord::Receipt(receipt(&id, "queued", "queued")),
        ];
        for record in records.drain(..) {
            expected
                .put_agent_record(&container, record.clone())
                .unwrap();
            store
                .update_head(&id, |head| {
                    TaskMut::put_agent_record(head, &container, record).map(drop)
                })
                .unwrap();
        }
        let mut committed = receipt(&id, "queued", "committed");
        committed.post_pending.push("effect:fixture".into());
        expected
            .put_agent_record(&container, NativeRecord::Receipt(committed.clone()))
            .unwrap();
        store
            .update_head(&id, |head| {
                head.put_agent_record(&container, NativeRecord::Receipt(committed))
                    .map(drop)
            })
            .unwrap();
        expected
            .edit_agent_head(&container, |head| {
                head.question_hold = Some("owner".into());
                Ok(())
            })
            .unwrap();
        store
            .update_head(&id, |head| {
                head.edit_agent_head(&container, |head| {
                    head.question_hold = Some("owner".into());
                    Ok(())
                })
            })
            .unwrap();
        let actual = store.get(&id).unwrap();
        assert_eq!(
            agent::container_state(&actual, &container),
            agent::container_state(&expected, &container)
        );
        let head = store.head(&id).unwrap();
        assert_eq!(
            head.agent_head(&container).unwrap().unwrap(),
            AgentHostHead::of(agent::container_state(&expected, &container).unwrap())
        );
        assert_eq!(head.pending_agent_receipts(&container).unwrap().len(), 1);
        assert_eq!(
            head.latest_agent_turn(&container, 10000)
                .unwrap()
                .unwrap()
                .seq,
            2
        );
        let mut finished = receipt(&id, "queued", "committed");
        finished.state = ReceiptState::Committed;
        store
            .update_head(&id, |head| {
                head.put_agent_record(&container, NativeRecord::Receipt(finished))
                    .map(drop)
            })
            .unwrap();
        assert!(
            store
                .head(&id)
                .unwrap()
                .pending_agent_receipts(&container)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn native_receipt_mutation_has_equal_work_with_grown_completed_history() {
    let mut results = Vec::new();
    for scale in [1, 10] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        let mut task = store_support::rich(&store, 1, 0, 0, 0);
        task.created = at(0);
        task.updated = at(0);
        task.status_since = Some(at(0));
        let container = ContainerRef::Worker {
            task: task.id.clone(),
            run: task.runs[0].id.clone(),
        };
        let mut native = state("worker");
        for n in 0..scale {
            native
                .receipts
                .push(receipt(&task.id, &format!("old-{n:04}"), "committed"));
        }
        native.receipts.push(receipt(&task.id, "queued", "queued"));
        task.runs[0].agent_host = Some(Box::new(native));
        store.fixture_put_task(&task).unwrap();
        let _hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(
            store
                .head(&task.id)
                .unwrap()
                .pending_agent_receipts(&container)
                .unwrap()
                .len(),
            1
        );
        store
            .update_head(&task.id, |head| {
                head.put_agent_record(
                    &container,
                    NativeRecord::Receipt(receipt(&task.id, "queued", "committed")),
                )?;
                head.put_agent_record(
                    &container,
                    NativeRecord::Receipt(receipt(&task.id, "new", "queued")),
                )?;
                Ok(())
            })
            .unwrap();
        let after = work::snapshot();
        results.push(
            [
                Counter::TaskHeadDecodes,
                Counter::NestedRowDecodes,
                Counter::HistoryRowDecodes,
                Counter::FullTaskHydrations,
                Counter::DecodedDocBytes,
                Counter::ViewsOpened,
            ]
            .map(|counter| after.total(counter) - before.total(counter)),
        );
    }
    assert_eq!(results[0], results[1]);
    assert_eq!(results[0][3], 0);
}
