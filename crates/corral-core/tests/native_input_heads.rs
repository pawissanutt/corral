//! Unfinished native owners are indexed independently of retained native/task history.
mod store_support;
use corral_core::agent::{
    AgentBinding, AgentRole, AgentRunState, ContainerRef, HostHealth, NativeRecord, QuestionState,
};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{Status, TaskRef};
use serde_json::json;

fn host(role: AgentRole) -> Box<AgentRunState> {
    Box::new(AgentRunState::new(AgentBinding {
        role,
        store: "fixture".into(),
        project: "app".into(),
        generation: 1,
        health: HostHealth::Unbound,
        callback: None,
        closing: false,
    }))
}

#[test]
fn unfinished_native_owner_order_and_retirement_match_full_without_decoding_settled_history() {
    let mut measured = Vec::new();
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
        for run in &mut task.runs {
            run.harness = Some(corral_core::CORRAL_PI_HARNESS.into());
            run.inbox.settled = Some(store_support::at(1));
            for child in &mut run.review_runs {
                child.harness = Some(corral_core::CORRAL_PI_HARNESS.into());
                child.inbox.settled = Some(store_support::at(1));
            }
        }
        for run in &mut task.plan_runs {
            run.harness = corral_core::CORRAL_PI_HARNESS.into();
            run.inbox.settled = Some(store_support::at(1));
        }
        for plan in &mut task.plans {
            for review in &mut plan.reviews {
                review.agent_host = Some(host(AgentRole::PlanReviewer));
                review.inbox.settled = Some(store_support::at(1));
            }
        }
        task.runs.last_mut().unwrap().inbox.settled = None;
        task.runs
            .last_mut()
            .unwrap()
            .review_runs
            .last_mut()
            .unwrap()
            .inbox
            .settled = None;
        task.plan_runs.last_mut().unwrap().inbox.settled = None;
        task.plans
            .last_mut()
            .unwrap()
            .reviews
            .last_mut()
            .unwrap()
            .inbox
            .settled = None;
        let old = &mut task.runs[0];
        let child = &mut old.review_runs[0];
        let container = ContainerRef::Reviewer {
            task: task.id.clone(),
            run: old.id.clone(),
            review: child.id.clone(),
        };
        let key = format!("{}/Q1", container.key());
        let mut state = host(AgentRole::Reviewer);
        state.questions.push(serde_json::from_value(json!({"version":1,"key":key,"revision":1,"kind":"clarification",
            "asked":{"request_id":"ask","payload_sha256":"00","session_id":"session","generation":1,"at":store_support::at(0)},
            "blocking":true,"text":"pending","state":"open","wake":{"route":"none","key":"wake","state":"pending"}})).unwrap());
        child.agent_host = Some(state);
        let expected = task.unfinished_native_containers().unwrap();
        assert_eq!(expected.len(), 5);
        assert_eq!(expected[0], container);
        store.fixture_put_task(&task).unwrap();
        let pinned = store.head(&task.id).unwrap();
        let hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(pinned.unfinished_native_containers().unwrap(), expected);
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|c| after.total(c) - before.total(c)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
        drop(hot);
        store
            .update_head(&task.id, |head| {
                assert!(TaskRef::unfinished_native_containers(head)?.contains(&container));
                let mut question = head.agent_question(&container, &key)?.unwrap();
                question.state = QuestionState::Cancelled;
                head.put_agent_record(&container, NativeRecord::Question(question))?;
                assert!(!TaskRef::unfinished_native_containers(head)?.contains(&container));
                Ok(())
            })
            .unwrap();
        assert_eq!(
            pinned.unfinished_native_containers().unwrap(),
            expected,
            "pinned view retains original inputs"
        );
        let fresh = store
            .head(&task.id)
            .unwrap()
            .unfinished_native_containers()
            .unwrap();
        assert_eq!(fresh.len(), 4);
        assert_eq!(
            fresh,
            store
                .get(&task.id)
                .unwrap()
                .unfinished_native_containers()
                .unwrap()
        );
    }
    assert_eq!(measured[0], measured[1]);
}
