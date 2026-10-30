//! Native record edits preserve full-facade state without hydrating native history.
mod store_support;
use corral_core::agent::{
    self, AgentRunState, ContainerRef, NativeRecord, ReceiptState, RequestReceipt,
};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{AgentHostHead, Store, TaskMut, TaskRef};
use serde_json::json;
use store_support::at;

#[test]
fn pm_receipt_effect_clear_is_keyed_and_history_independent() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        let mut task = store_support::rich(&store, 1, 0, 0, 0);
        task.created = at(0);
        task.updated = at(0);
        task.status_since = Some(at(0));
        task.agent_pm_receipts = (0..scale)
            .map(|n| receipt(&task.id, &format!("old-{n:04}"), "committed"))
            .collect();
        let mut target = receipt(&task.id, "target", "committed");
        target.post_pending = vec!["effect:event".into()];
        task.agent_pm_receipts.push(target.clone());
        store.fixture_put_task(&task).unwrap();
        {
            let _hot = HotScope::enter();
            let before = work::snapshot();
            assert!(
                store
                    .try_update_head(&task.id, |head| head.edit_pm_receipt("target", |receipt| {
                        receipt.post_pending.clear();
                        Ok(())
                    }))
                    .unwrap()
                    .is_some()
            );
            assert!(
                store
                    .try_update_head(&task.id, |head| head
                        .edit_pm_receipt("absent", |_| anyhow::bail!(
                            "missing receipt must not invoke editor"
                        )))
                    .unwrap()
                    .is_none()
            );
            let after = work::snapshot();
            measured.push(
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
        target.post_pending.clear();
        let full = store.get(&task.id).unwrap();
        assert_eq!(full.agent_pm_receipts.last(), Some(&target));
        assert_eq!(
            &full.agent_pm_receipts[..scale],
            &task.agent_pm_receipts[..scale]
        );
    }
    assert_eq!(measured[0], measured[1]);
    assert_eq!(measured[0][3], 0);
}

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
            // Bridge events can arrive out of order: latest means max seq, not last append.
            turn(10000, 2),
            turn(10000, 1),
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
fn unfinished_native_index_skips_settled_terminal_tasks_and_retains_owed_receipts() {
    let mut results = Vec::new();
    for scale in [1, 10] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        let mut task = store
            .create("Owed receipt", None, corral_core::Status::Done, "")
            .unwrap();
        for _ in 0..scale {
            let mut task = store
                .create("Retired", None, corral_core::Status::Done, "")
                .unwrap();
            task.runs = vec![store_support::coding_run(&task.id, "R0001", 0, 0, 0, true)];
            task.runs[0].inbox.settled = Some(at(1));
            task.runs[0].agent_host = Some(Box::new(state("worker")));
            store.fixture_put_task(&task).unwrap();
        }
        task.created = at(0);
        task.updated = at(0);
        task.status_since = Some(at(0));
        task.runs = vec![store_support::coding_run(&task.id, "R0001", 0, 0, 0, true)];
        task.runs[0].inbox.settled = Some(at(1));
        let mut native = state("worker");
        native.receipts.push(receipt(&task.id, "queued", "queued"));
        task.runs[0].agent_host = Some(Box::new(native));
        store.fixture_put_task(&task).unwrap();
        let container = ContainerRef::Worker {
            task: task.id.clone(),
            run: "R0001".into(),
        };
        {
            let _hot = HotScope::enter();
            let before = work::snapshot();
            let heads = store.unfinished_native_heads().unwrap();
            assert_eq!(heads.iter().map(|h| &h.id).collect::<Vec<_>>(), [&task.id]);
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
        store
            .update_head(&task.id, |head| {
                head.put_agent_record(
                    &container,
                    NativeRecord::Receipt(receipt(&task.id, "queued", "committed")),
                )
                .map(drop)
            })
            .unwrap();
        assert!(store.unfinished_native_heads().unwrap().is_empty());
        let recipient = corral_core::inbox::Recipient::Run {
            task: task.id.clone(),
            run: "R0001".into(),
        };
        store
            .update_head(&task.id, |head| {
                head.inbox_state_mut(&recipient, |inbox| {
                    inbox.settled = None;
                    Ok(())
                })
            })
            .unwrap();
        assert_eq!(store.unfinished_native_heads().unwrap().len(), 1);
        store
            .update_head(&task.id, |head| {
                head.inbox_state_mut(&recipient, |inbox| {
                    inbox.settled = Some(at(1));
                    Ok(())
                })
            })
            .unwrap();
        assert!(store.unfinished_native_heads().unwrap().is_empty());
    }
    assert_eq!(results[0], results[1]);
    assert_eq!(results[0][3], 0);
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
