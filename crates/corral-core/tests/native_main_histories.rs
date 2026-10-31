//! Incoming-main histories must survive real full/head writes without entering host headers.
mod store_support;
use corral_core::agent::{self, AgentRunState, ContainerRef, NativeRecord};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{AgentHostHead, Store, TaskMut, TaskRef};
use serde_json::json;
use store_support::at;

fn records(n: usize) -> [NativeRecord; 3] {
    [
        NativeRecord::Rewind(
            serde_json::from_value(json!({
                "id": format!("rw-{n:04}"), "checkpoint":"g1-t1", "commit":"commit",
                "head":"head", "leaf":"leaf", "from_generation":1, "stage":"aborted",
                "at":at(0), "updated":at(1)
            }))
            .unwrap(),
        ),
        NativeRecord::UsageSession(
            serde_json::from_value(json!({
                "generation":1, "kind":"oracle", "session_id":format!("usage-{n:04}"),
                "session_file":"/fixture/usage.jsonl", "model":"fixture-model",
                "request":format!("usage-{n:04}")
            }))
            .unwrap(),
        ),
        NativeRecord::BudgetRequest(
            serde_json::from_value(json!({
                "request":format!("budget-{n:04}"), "generation":1, "tokens":100,
                "usd_nanos":1000, "priced":true, "response":format!("response-{n:04}")
            }))
            .unwrap(),
        ),
    ]
}

#[test]
fn incoming_histories_roundtrip_all_task_roles_and_head_work_is_history_independent() {
    let mut budgets = Vec::new();
    for scale in [1, 10] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        store.fixture_pin_now(at(10)).unwrap();
        let mut expected = store_support::rich(&store, 2, 2, 2, 2);
        let id = expected.id.clone();
        let run = expected.runs.last().unwrap();
        let plan = expected.plans.last().unwrap();
        let cases = [
            (
                ContainerRef::Worker {
                    task: id.clone(),
                    run: run.id.clone(),
                },
                "worker",
            ),
            (
                ContainerRef::Reviewer {
                    task: id.clone(),
                    run: run.id.clone(),
                    review: run.review_runs.last().unwrap().id.clone(),
                },
                "reviewer",
            ),
            (
                ContainerRef::Planner {
                    task: id.clone(),
                    run: expected.plan_runs.last().unwrap().id.clone(),
                },
                "planner",
            ),
            (
                ContainerRef::PlanReviewer {
                    task: id.clone(),
                    plan: plan.id.clone(),
                    review: plan.reviews.last().unwrap().id.clone(),
                },
                "plan_reviewer",
            ),
        ];
        for (container, role) in &cases {
            let state = AgentRunState::new(
                serde_json::from_value(json!({
                    "role":role, "store":"st-fixture", "project":"app", "generation":1
                }))
                .unwrap(),
            );
            expected
                .init_agent_head(container, AgentHostHead::of(&state))
                .unwrap();
            for n in 0..scale {
                for record in records(n) {
                    expected.put_agent_record(container, record).unwrap();
                }
            }
        }
        store.fixture_put_task(&expected).unwrap();
        // Exercise the full split/assembly path before any keyed mutation.
        let initial = store.get(&id).unwrap();
        for (container, _) in &cases {
            assert_eq!(
                agent::container_state(&initial, container),
                agent::container_state(&expected, container)
            );
        }
        let delta;
        {
            let _hot = HotScope::enter();
            let before = work::snapshot();
            store
                .update_head(&id, |head| {
                    for (container, _) in &cases {
                        head.edit_agent_head(container, |host| {
                            host.question_hold = Some("held".into());
                            Ok(())
                        })?;
                        for record in records(100) {
                            head.put_agent_record(container, record)?;
                        }
                    }
                    Ok(())
                })
                .unwrap();
            let after = work::snapshot();
            delta = after.since(&before);
        }
        delta.check_identity().unwrap();
        assert_eq!(delta.total(Counter::FullTaskHydrations), 0);
        budgets.push(delta);
        for (container, _) in &cases {
            expected
                .edit_agent_head(container, |host| {
                    host.question_hold = Some("held".into());
                    Ok(())
                })
                .unwrap();
            for record in records(100) {
                expected.put_agent_record(container, record).unwrap();
            }
        }
        // A cold full save must retain all history too, including budget's nested JSON shape.
        store
            .update(&id, |task| {
                task.title.push_str(" updated");
                Ok(())
            })
            .unwrap();
        drop(store);
        let reopened = Store::open(root.path()).unwrap();
        let actual = reopened.get(&id).unwrap();
        for (container, _) in &cases {
            let state = agent::container_state(&actual, container).unwrap();
            assert_eq!(Some(state), agent::container_state(&expected, container));
            assert_eq!(state.rewinds.len(), scale + 1);
            assert_eq!(state.usage_sessions.len(), scale + 1);
            assert_eq!(state.budget.requests.len(), scale + 1);
            let head = reopened
                .head(&id)
                .unwrap()
                .agent_head(container)
                .unwrap()
                .unwrap();
            assert_eq!(&head.records.counts[7..], &[scale as u64 + 1; 3]);
        }
    }
    assert_eq!(budgets[0], budgets[1]);
}

#[test]
fn native_footer_reads_legacy_seven_but_rejects_partial_or_malformed_counts() {
    let legacy = json!({"counts":[1,2,3,4,5,6,7], "pending":0});
    let parsed: corral_core::NativeRecordCounts = serde_json::from_value(legacy).unwrap();
    assert_eq!(parsed.counts, [1, 2, 3, 4, 5, 6, 7, 0, 0, 0]);
    assert_eq!(
        serde_json::to_value(parsed).unwrap()["counts"]
            .as_array()
            .unwrap()
            .len(),
        10
    );
    for counts in [
        json!([1, 2, 3, 4, 5, 6, 7, 8]),
        json!([1, 2, 3, 4, 5, 6, "bad"]),
    ] {
        assert!(
            serde_json::from_value::<corral_core::NativeRecordCounts>(
                json!({"counts":counts,"pending":0})
            )
            .is_err()
        );
    }
}
