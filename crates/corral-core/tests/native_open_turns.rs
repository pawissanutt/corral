//! Open-turn existence is generation-scoped and independent of older lost/open history.
mod store_support;
use corral_core::TaskRef;
use corral_core::agent::{
    AgentBinding, AgentRole, AgentRunState, ContainerRef, HostHealth, NativeRecord,
};
use corral_core::work::{self, Counter, HotScope};
use serde_json::json;

#[test]
fn current_generation_open_turn_is_not_inferred_from_the_latest_turn_or_older_generations() {
    for role in [
        AgentRole::Worker,
        AgentRole::Reviewer,
        AgentRole::Planner,
        AgentRole::PlanReviewer,
    ] {
        for open in [false, true] {
            let mut measured = Vec::new();
            for scale in [1, 10] {
                let (_dir, store) = store_support::store();
                let mut task = store_support::rich(
                    &store,
                    2 * scale + 1,
                    2 * scale + 1,
                    2 * scale + 1,
                    2 * scale,
                );
                let mut host = AgentRunState::new(AgentBinding {
                    role,
                    store: "fixture".into(),
                    project: "app".into(),
                    generation: 2,
                    health: HostHealth::Healthy,
                    callback: None,
                    closing: false,
                });
                let turn = |generation: u32, seq: u64, ended: bool| {
                    serde_json::from_value(json!({"generation":generation,"seq":seq,
                    "origin":{"kind":"unknown"},"started":store_support::at(0),"ended":ended.then(||store_support::at(1)),"class":null,"provider_error":null})).unwrap()
                };
                for n in 0..2 * scale {
                    host.turns.push(turn(1, n as u64, false));
                }
                host.turns.push(turn(1, 999, true));
                host.turns.push(turn(2, 7, true));
                host.turns.push(turn(2, 3, !open));
                host.turns.push(turn(2, 9, true));
                let container = match role {
                    AgentRole::Worker => {
                        let run = task.runs.last_mut().unwrap();
                        run.agent_host = Some(Box::new(host));
                        ContainerRef::Worker {
                            task: task.id.clone(),
                            run: run.id.clone(),
                        }
                    }
                    AgentRole::Reviewer => {
                        let run = task.runs.last_mut().unwrap();
                        let review = run.review_runs.last_mut().unwrap();
                        review.agent_host = Some(Box::new(host));
                        ContainerRef::Reviewer {
                            task: task.id.clone(),
                            run: run.id.clone(),
                            review: review.id.clone(),
                        }
                    }
                    AgentRole::Planner => {
                        let run = task.plan_runs.last_mut().unwrap();
                        run.agent_host = Some(Box::new(host));
                        ContainerRef::Planner {
                            task: task.id.clone(),
                            run: run.id.clone(),
                        }
                    }
                    AgentRole::PlanReviewer => {
                        let plan = task.plans.last_mut().unwrap();
                        let review = plan.reviews.last_mut().unwrap();
                        review.agent_host = Some(Box::new(host));
                        ContainerRef::PlanReviewer {
                            task: task.id.clone(),
                            plan: plan.id.clone(),
                            review: review.id.clone(),
                        }
                    }
                    AgentRole::Pm | AgentRole::Assistant => unreachable!(),
                };
                assert_eq!(task.has_open_agent_turn(&container, 2).unwrap(), open);
                store.fixture_put_task(&task).unwrap();
                let head = store.head(&task.id).unwrap();
                assert_eq!(
                    head.latest_agent_turn(&container, 2).unwrap().unwrap().seq,
                    9
                );
                let hot = HotScope::enter();
                let before = work::snapshot();
                assert_eq!(head.has_open_agent_turn(&container, 2).unwrap(), open);
                assert!(!head.has_open_agent_turn(&container, 999).unwrap());
                let latest = head.latest_agent_turn_any_generation(&container).unwrap();
                assert_eq!(
                    latest,
                    task.latest_agent_turn_any_generation(&container).unwrap()
                );
                assert_eq!(latest.unwrap().generation, 2);
                let ended = head
                    .agent_turn_ended_at(&container, 2, store_support::at(1))
                    .unwrap();
                assert_eq!(
                    ended,
                    task.agent_turn_ended_at(&container, 2, store_support::at(1))
                        .unwrap()
                );
                assert_eq!(
                    ended.unwrap().seq,
                    7,
                    "first append, neither maximum nor minimum sequence"
                );
                assert!(
                    head.agent_turn_ended_at(&container, 2, store_support::at(2))
                        .unwrap()
                        .is_none()
                );
                let after = work::snapshot();
                measured.push(Counter::ALL.map(|c| after.total(c) - before.total(c)));
                assert_eq!(
                    after.total(Counter::FullTaskHydrations)
                        - before.total(Counter::FullTaskHydrations),
                    0
                );
                drop(hot);
                if open {
                    store
                        .update_head(&task.id, |head| {
                            let mut changed = TaskRef::agent_turn(head, &container, 2, 3)?.unwrap();
                            changed.ended = Some(store_support::at(1));
                            head.put_agent_record(&container, NativeRecord::Turn(changed))?;
                            assert!(!TaskRef::has_open_agent_turn(head, &container, 2)?);
                            let mut moved = TaskRef::agent_turn(head, &container, 2, 7)?.unwrap();
                            moved.ended = Some(store_support::at(2));
                            head.put_agent_record(&container, NativeRecord::Turn(moved))?;
                            assert_eq!(
                                TaskRef::agent_turn_ended_at(
                                    head,
                                    &container,
                                    2,
                                    store_support::at(1)
                                )?
                                .unwrap()
                                .seq,
                                3
                            );
                            assert_eq!(
                                TaskRef::agent_turn_ended_at(
                                    head,
                                    &container,
                                    2,
                                    store_support::at(2)
                                )?
                                .unwrap()
                                .seq,
                                7
                            );
                            Ok(())
                        })
                        .unwrap();
                    assert!(
                        !store
                            .head(&task.id)
                            .unwrap()
                            .has_open_agent_turn(&container, 2)
                            .unwrap()
                    );
                }
                store.update_head(&task.id,|head| {
                    head.edit_agent_head(&container,|host|{host.binding.generation=3;host.binding.health=HostHealth::Unbound;Ok(())})?;
                    head.put_agent_record(&container,NativeRecord::Generation(serde_json::from_value(json!({"n":3,"bind_nonce_sha256":"00".repeat(32),"pane":"p1","created":store_support::at(3)}))?))?;
                    Ok(())
                }).unwrap();
                let next = store.head(&task.id).unwrap();
                assert!(next.latest_agent_turn(&container, 3).unwrap().is_none());
                let prior = next
                    .latest_agent_turn_any_generation(&container)
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    (prior.generation, prior.seq),
                    (2, 9),
                    "a new empty generation must not erase latest-turn evidence"
                );
            }
            assert_eq!(measured[0], measured[1], "role={role:?} open={open}");
        }
    }
}
