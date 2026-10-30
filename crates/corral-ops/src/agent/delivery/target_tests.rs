//! Real delivery facts must not assemble native or surrounding task history.
use super::*;
use corral_core::agent::{AgentRole, AgentRunState};
use corral_core::work::{self, Counter, HotScope};

#[path = "../../../../corral-core/tests/store_support/mod.rs"]
mod data;

fn host(role: AgentRole, n: usize) -> AgentRunState {
    let at = data::at(0);
    let mut state = AgentRunState::new(AgentBinding {
        role,
        store: "fixture".into(),
        project: "app".into(),
        generation: 999,
        health: HostHealth::Healthy,
        callback: None,
        closing: false,
    });
    let generation = |n| {
        serde_json::from_value(json!({"n":n,"bind_nonce_sha256":"00".repeat(32),"pane":"p1","created":at,"bound":{
        "host_cap_sha256":"11".repeat(32),"ctl_cap_sha256":"22".repeat(32),"session_id":"fixed-session","session_file":"/tmp/native-delivery-session.jsonl",
        "pid":3,"start_time":"1234","request_id":"hello","at":at}})).unwrap()
    };
    for i in 1..=n {
        state.generations.push(generation(i as u32));
        state.turns.push(serde_json::from_value(json!({"generation":i,"seq":1,"origin":{"kind":"unknown"},"started":at,"ended":at,"class":"completed","provider_error":null})).unwrap());
    }
    state.generations.push(generation(999));
    state
}

#[test]
fn native_delivery_binding_and_closure_match_full_state_with_constant_work_for_every_task_role() {
    for role in [
        AgentRole::Worker,
        AgentRole::Reviewer,
        AgentRole::Planner,
        AgentRole::PlanReviewer,
    ] {
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
            let state = host(role, 2 * scale);
            let container = match role {
                AgentRole::Worker => {
                    let run = task.runs.last_mut().unwrap();
                    run.agent_host = Some(Box::new(state));
                    ContainerRef::Worker {
                        task: task.id.clone(),
                        run: run.id.clone(),
                    }
                }
                AgentRole::Reviewer => {
                    let run = task.runs.last_mut().unwrap();
                    let child = run.review_runs.last_mut().unwrap();
                    child.agent_host = Some(Box::new(state));
                    ContainerRef::Reviewer {
                        task: task.id.clone(),
                        run: run.id.clone(),
                        review: child.id.clone(),
                    }
                }
                AgentRole::Planner => {
                    let run = task.plan_runs.last_mut().unwrap();
                    run.agent_host = Some(Box::new(state));
                    ContainerRef::Planner {
                        task: task.id.clone(),
                        run: run.id.clone(),
                    }
                }
                AgentRole::PlanReviewer => {
                    let plan = task.plans.last_mut().unwrap();
                    let review = plan.reviews.last_mut().unwrap();
                    review.agent_host = Some(Box::new(state));
                    ContainerRef::PlanReviewer {
                        task: task.id.clone(),
                        plan: plan.id.clone(),
                        review: review.id.clone(),
                    }
                }
                AgentRole::Pm => unreachable!(),
            };
            let recipient = crate::questions::recipient(&container).unwrap();
            let expected = corral_core::agent::container_state(&task, &container).unwrap();
            let binding = expected.binding.clone();
            let generation = expected.current_generation().cloned();
            let closed =
                corral_core::agent::container_closure(&task, &container).map(|r| r.as_str());
            store.fixture_put_task(&task).unwrap();
            let ctx = Ctx::from_store(store);
            // Observer negative control: an accidental compatibility read must be visible.
            let hot = HotScope::enter();
            let before = work::snapshot();
            let _full = ctx.store.get(&task.id).unwrap();
            let after = work::snapshot();
            assert_eq!(
                after.total(Counter::FullTaskHydrations)
                    - before.total(Counter::FullTaskHydrations),
                1
            );
            drop(hot);
            let hot = HotScope::enter();
            let before = work::snapshot();
            let selected = target(&ctx, &recipient)
                .unwrap()
                .unwrap_or_else(|reason| panic!("{reason}"));
            assert_eq!(selected.binding, binding);
            assert_eq!(selected.generation, generation);
            assert_eq!(selected.closed, closed);
            assert!(unready(&selected).is_none());
            assert_eq!(
                session(&ctx, &recipient, &selected)
                    .unwrap()
                    .unwrap()
                    .session,
                "fixed-session"
            );
            let after = work::snapshot();
            measured.push(Counter::ALL.map(|c| after.total(c) - before.total(c)));
            assert_eq!(
                after.total(Counter::FullTaskHydrations)
                    - before.total(Counter::FullTaskHydrations),
                0
            );
            drop(hot);
        }
        assert_eq!(measured[0], measured[1], "role={role:?}");
    }
}
