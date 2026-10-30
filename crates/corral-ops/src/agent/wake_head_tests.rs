//! Actual wake publication/cancellation and replay must not assemble native or wait history.
use super::*;
use crate::agent::native_test_support::{data, host};
use corral_core::Status;
use corral_core::agent::{AgentRole, RequestReceipt};
use corral_core::work::{self, Counter, HotScope};

#[test]
fn wait_and_gate_wakes_post_once_or_cancel_with_constant_history_work() {
    for case in ["wait", "gate-pass", "gate-refused", "closed-wait"] {
        let mut measured = Vec::new();
        for scale in [1, 10] {
            let (_dir, store) = data::store();
            store.fixture_pin_now(data::at(100_000)).unwrap();
            let mut task = data::rich(
                &store,
                2 * scale + 1,
                2 * scale + 1,
                2 * scale + 1,
                2 * scale,
            );
            let closed = case == "closed-wait";
            task.status = if closed {
                Status::Done
            } else {
                Status::Running
            };
            let n = if closed { 0 } else { task.runs.len() - 1 };
            let run = &mut task.runs[n];
            run.outcome = if closed {
                Some(corral_core::RunOutcome::Done)
            } else {
                None
            };
            let container = ContainerRef::Worker {
                task: task.id.clone(),
                run: run.id.clone(),
            };
            let mut state = host(AgentRole::Worker, 2 * scale);
            for i in 0..2 * scale {
                state.wakes.push(serde_json::from_value(json!({"key":format!("old-{i:04}"),"request_id":format!("old-{i:04}"),"kind":"wait","target":"old","state":"fired","at":data::at(0)})).unwrap());
            }
            let gate = case.starts_with("gate");
            state.wakes.push(serde_json::from_value(json!({"key":"active","request_id":"active","kind":if gate {"gate"}else {"wait"},"target":if gate {"attempt-fixed"}else {"W0999"},"state":"armed","at":data::at(0)})).unwrap());
            let receipt:RequestReceipt=serde_json::from_value(json!({"version":1,"request_id":"queued","op":"gate","payload_sha256":"hash",
                "identity":{"store":"fixture","project":"app","role":"worker","task":task.id,"run":run.id,"session_id":"session","generation":999},
                "state":"committed","at":data::at(0),"effect":{"kind":"gate_attempt","ref":"attempt-fixed"},"result":{"queued":true,"reason":"fixture refusal"}})).unwrap();
            if case == "gate-refused" {
                let mut rejected = receipt.clone();
                rejected.request_id = "active".into();
                rejected.state = agent::ReceiptState::Rejected;
                rejected.result = json!({"queued":true, "reason":"fixture refusal"});
                state.receipts.push(rejected);
            }
            state.receipts.push(receipt);
            if case == "gate-pass" {
                let mut identity = data::identity(&task.id);
                identity["run"] = json!(run.id);
                identity["attempt"] = json!("attempt-fixed");
                run.gate = Some(
                    serde_json::from_value(json!({"identity":identity,
                    "result":"pass", "at":data::at(5), "sha":"1".repeat(40)}))
                    .unwrap(),
                );
            }
            run.agent_host = Some(Box::new(state));
            run.obligations
                .waits
                .push(serde_json::from_value(data::wait(&run.id, 999, true)).unwrap());
            let expected = due_wait_wakes(&task);
            let expected_gate = crate::agent::effects::due_gate_wakes(
                &task,
                data::at(100_000),
                std::time::Duration::from_secs(1),
            );
            store.fixture_put_task(&task).unwrap();
            let ctx = Ctx::from_store(store);
            let hot = HotScope::enter();
            let before = work::snapshot();
            let head = ctx.store.head(&task.id).unwrap();
            let due = if gate {
                let actual = crate::review_budget::due_gate_wakes_ref(
                    &head,
                    data::at(100_000),
                    std::time::Duration::from_secs(1),
                )
                .unwrap();
                assert_eq!(actual, expected_gate);
                assert_eq!(actual.len(), 1);
                assert_eq!(
                    actual[0].reason,
                    if case == "gate-refused" {
                        "not_started"
                    } else {
                        "pass"
                    }
                );
                actual
            } else {
                let actual = due_wait_wakes_ref(&head).unwrap();
                assert_eq!(actual, expected);
                actual
            };
            assert_eq!(post_due(&ctx, &due).unwrap(), usize::from(!closed));
            assert_eq!(post_due(&ctx, &due).unwrap(), 0);
            let after = work::snapshot();
            measured.push(Counter::ALL.map(|c| after.total(c) - before.total(c)));
            assert_eq!(
                after.total(Counter::FullTaskHydrations)
                    - before.total(Counter::FullTaskHydrations),
                0
            );
            drop(hot);
            let fresh = ctx.store.head(&task.id).unwrap();
            let stored = fresh.agent_wake(&container, "active").unwrap().unwrap();
            assert_eq!(
                stored.state,
                if closed {
                    WakeState::Cancelled
                } else {
                    WakeState::Fired
                }
            );
            let messages = Inbox::new(&ctx.store, crate::questions::recipient(&container).unwrap())
                .unwrap()
                .messages()
                .unwrap();
            assert_eq!(messages.len(), usize::from(!closed));
            if !closed {
                assert_eq!(stored.fired_at, Some(data::at(100_000)));
                assert_eq!(messages[0].kind, WAKE_KIND);
                assert!(messages[0].body.contains(if case == "gate-refused" {
                    "fixture refusal"
                } else if gate {
                    "finished: pass"
                } else {
                    "Wait W0999"
                }));
            }
        }
        assert_eq!(measured[0], measured[1], "case={case}");
    }
}
