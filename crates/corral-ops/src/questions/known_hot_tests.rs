//! Native question replay must be independent of settled question and event history.
use super::*;
use crate::agent::native_test_support::data;
use corral_core::agent::AgentRole;
use corral_core::work::{self, Counter, HotScope};

#[test]
fn question_event_replay_reads_only_owed_transitions() {
    for scale in [1, 2] {
        let (_dir, store) = data::store();
        store.fixture_pin_now(data::at(100_000)).unwrap();
        let mut task = data::rich(&store, 1, 0, 0, 0);
        let run = task.runs.last_mut().unwrap();
        let container = ContainerRef::Worker {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        run.agent_host = Some(super::heads_tests::questions(
            &container,
            AgentRole::Worker,
            2 * scale,
        ));
        store.fixture_put_task(&task).unwrap();
        assert_eq!(reconcile_events(&store, &task.id).unwrap(), 2 * scale + 1);
        for n in 0..2 * scale {
            store
                .log_event(None, "unrelated_fixture_event", serde_json::json!({"n": n}))
                .unwrap();
        }
        let events = store.events().unwrap();
        let hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(reconcile_events(&store, &task.id).unwrap(), 0);
        let delta = work::snapshot().since(&before);
        drop(hot);
        delta.check_identity().unwrap();
        assert!(delta.followup_calls.is_empty());
        for counter in Counter::ALL {
            assert_eq!(
                delta.total(counter),
                u64::from(counter == Counter::ViewsOpened),
                "{}",
                counter.name()
            );
        }
        assert_eq!(store.events().unwrap(), events);
    }
}

#[test]
fn automatic_deadline_transition_has_history_independent_work() {
    let mut measured = Vec::new();
    for scale in [1, 2] {
        let (_dir, store) = data::store();
        store.fixture_pin_now(data::at(100_000)).unwrap();
        let mut task = data::rich(&store, 1, 0, 0, 0);
        task.status = corral_core::Status::Running;
        let run = task.runs.last_mut().unwrap();
        run.outcome = None;
        run.review = None;
        let container = ContainerRef::Worker {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        let mut state = super::heads_tests::questions(&container, AgentRole::Worker, 2 * scale);
        // Grow only question history, not unrelated current-head usage/summary scalars.
        state.generations.retain(|generation| generation.n == 999);
        state.turns.clear();
        let question = state.questions.last_mut().unwrap();
        let key = question.key.clone();
        question.deadline = Some(serde_json::from_value(serde_json::json!({"set_by":"owner","at":data::at(1),"for_revision":1,"instruction":"Use fixed payload."})).unwrap());
        run.agent_host = Some(state);
        store.fixture_put_task(&task).unwrap();
        reconcile_events(&store, &task.id).unwrap();
        let ctx = Ctx::from_store(store);
        let hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(
            expire_deadlines(&ctx, &task.id, data::at(100_000)).unwrap(),
            vec![key.clone()]
        );
        assert!(
            expire_deadlines(&ctx, &task.id, data::at(100_000))
                .unwrap()
                .is_empty()
        );
        let delta = work::snapshot().since(&before);
        drop(hot);
        delta.check_identity().unwrap();
        assert!(delta.followup_calls.is_empty());
        assert_eq!(delta.total(Counter::FullTaskHydrations), 0);
        assert_eq!(delta.total(Counter::EventDecodes), 0);
        measured.push(Counter::ALL.map(|counter| delta.total(counter)));
        assert_eq!(
            ctx.store
                .head(&task.id)
                .unwrap()
                .agent_question(&container, &key)
                .unwrap()
                .unwrap()
                .state,
            QuestionState::Expired
        );
        let messages = corral_core::inbox::Inbox::new(&ctx.store, recipient(&container).unwrap())
            .unwrap()
            .messages()
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].kind, "question_answer");
    }
    assert_eq!(measured[0], measured[1]);
}
