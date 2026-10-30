//! OD11's real producer and replay, with growing old generations, open lost turns and task history.
use super::*;
use crate::agent::native_test_support::{data, host};
use crate::{Ctx, agent};
use corral_core::Status;
use corral_core::agent::{AgentRole, GenerationFence};
use corral_core::inbox::Inbox;
use corral_core::work::{self, Counter, HotScope};
use serde_json::json;

#[test]
fn worker_and_reviewer_nudge_commit_once_with_constant_native_history_work() {
    for role in [AgentRole::Worker, AgentRole::Reviewer] {
        let mut measured = Vec::new();
        for scale in [1, 10] {
            let (_dir, store) = data::store();
            store.fixture_pin_now(data::at(100_000)).unwrap();
            for _ in 0..2 * scale {
                store.create("Terminal", None, Status::Done, "").unwrap();
            }
            let mut task = data::rich(
                &store,
                2 * scale + 1,
                2 * scale + 1,
                2 * scale + 1,
                2 * scale,
            );
            let mut state = host(role, 2 * scale);
            for generation in state.generations.iter_mut().filter(|g| g.n != 999) {
                generation.fenced = Some(GenerationFence {
                    reason: "replaced".into(),
                    at: data::at(0),
                });
            }
            for turn in &mut state.turns {
                turn.ended = None;
                turn.class = None;
            }
            let class = if role == AgentRole::Worker {
                TurnClass::NoReport
            } else {
                TurnClass::NoVerdict
            };
            state.turns.push(serde_json::from_value(json!({"generation":999,"seq":9,"origin":{"kind":"unknown"},"started":data::at(0),"ended":data::at(1),"class":class,"provider_error":null})).unwrap());
            let run = task.runs.last_mut().unwrap();
            run.review = None;
            let container = if role == AgentRole::Worker {
                run.outcome = None;
                task.status = Status::Running;
                run.agent_host = Some(Box::new(state));
                ContainerRef::Worker {
                    task: task.id.clone(),
                    run: run.id.clone(),
                }
            } else {
                run.outcome = Some(corral_core::RunOutcome::Done);
                task.status = Status::Review;
                let child = run.review_runs.last_mut().unwrap();
                child.outcome = None;
                child.review = None;
                child.agent_host = Some(Box::new(state));
                ContainerRef::Reviewer {
                    task: task.id.clone(),
                    run: run.id.clone(),
                    review: child.id.clone(),
                }
            };
            store.fixture_put_task(&task).unwrap();
            let ctx = Ctx::from_store(store);
            let recipient = crate::questions::recipient(&container).unwrap();
            let hot = HotScope::enter();
            let before = work::snapshot();
            assert!(agent::turns::nudge(&ctx, &task.id).unwrap());
            assert!(
                !agent::turns::nudge(&ctx, &task.id).unwrap(),
                "no duplicate nudge on replay"
            );
            let head = ctx.store.head(&task.id).unwrap();
            assert_eq!(
                head.agent_generation(&container, 999)
                    .unwrap()
                    .unwrap()
                    .nudged_at,
                Some(data::at(1))
            );
            if role == AgentRole::Worker {
                assert!(agent::turns::attention_ref(&head).unwrap().is_none());
            } else {
                assert!(agent::hooks::review::between_silent_turns_ref(&head).unwrap());
            }
            let after = work::snapshot();
            measured.push(Counter::ALL.map(|c| after.total(c) - before.total(c)));
            assert_eq!(
                after.total(Counter::FullTaskHydrations)
                    - before.total(Counter::FullTaskHydrations),
                0
            );
            drop(hot);
            let messages = Inbox::new(&ctx.store, recipient)
                .unwrap()
                .messages()
                .unwrap();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].kind, agent::turns::NUDGE_KIND);
            assert_eq!(messages[0].subject.as_ref().unwrap()["turn"], 9);
            if role == AgentRole::Worker {
                assert!(agent::turns::attention(&ctx.store.get(&task.id).unwrap()).is_none());
            }
        }
        assert_eq!(measured[0], measured[1], "role={role:?}");
    }
}
