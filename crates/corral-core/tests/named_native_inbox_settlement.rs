//! Explicit named native inbox settlement preserves historical metadata and retires owed work.
mod store_support;
use corral_core::{
    PlanRunState, RunOutcome, TaskRef, Verdict,
    agent::{AgentBinding, AgentRole, AgentRunState, ContainerRef, HostHealth},
    inbox::Recipient,
    work::{self, Counter, HotScope},
};

fn recipient(container: &ContainerRef) -> Recipient {
    match container {
        ContainerRef::Worker { task, run } => Recipient::Run {
            task: task.clone(),
            run: run.clone(),
        },
        ContainerRef::Reviewer { task, run, review } => Recipient::ReviewRun {
            task: task.clone(),
            run: run.clone(),
            review: review.clone(),
        },
        ContainerRef::Planner { task, run } => Recipient::PlanRun {
            task: task.clone(),
            plan_run: run.clone(),
        },
        ContainerRef::PlanReviewer { task, plan, review } => Recipient::PlanReview {
            task: task.clone(),
            plan: plan.clone(),
            review: review.clone(),
        },
        ContainerRef::Pm => unreachable!(),
    }
}

#[test]
fn ten_named_current_and_historical_native_inboxes_settle_once_with_exact_growth_work() {
    let mut costs = Vec::new();
    for scale in [1, 10] {
        let (_dir, store) = store_support::store();
        let at = store_support::at(100_000);
        store.fixture_pin_now(at).unwrap();
        let mut task = store_support::rich(
            &store,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale,
        );
        let id = task.id.clone();
        let last = task.runs.len() - 1;
        let plan_last = task.plans.len() - 1;
        let planner_last = task.plan_runs.len() - 1;
        let mut targets = Vec::new();
        for n in [0, last] {
            task.runs[n].outcome = Some(RunOutcome::Done);
            targets.push(ContainerRef::Worker {
                task: id.clone(),
                run: task.runs[n].id.clone(),
            });
        }
        for (p, first) in [(0, false), (last, true), (last, false)] {
            let c = if first {
                0
            } else {
                task.runs[p].review_runs.len() - 1
            };
            task.runs[p].review_runs[c].outcome = Some(RunOutcome::Done);
            targets.push(ContainerRef::Reviewer {
                task: id.clone(),
                run: task.runs[p].id.clone(),
                review: task.runs[p].review_runs[c].id.clone(),
            });
        }
        for n in [0, planner_last] {
            task.plan_runs[n].state = PlanRunState::Cancelled;
            targets.push(ContainerRef::Planner {
                task: id.clone(),
                run: task.plan_runs[n].id.clone(),
            });
        }
        for (p, first) in [(0, false), (plan_last, true), (plan_last, false)] {
            let c = if first {
                0
            } else {
                task.plans[p].reviews.len() - 1
            };
            task.plans[p].reviews[c].verdict = Some(Verdict::Approve);
            targets.push(ContainerRef::PlanReviewer {
                task: id.clone(),
                plan: task.plans[p].id.clone(),
                review: task.plans[p].reviews[c].id.clone(),
            });
        }
        for container in &targets {
            let role = match container {
                ContainerRef::Worker { .. } => AgentRole::Worker,
                ContainerRef::Reviewer { .. } => AgentRole::Reviewer,
                ContainerRef::Planner { .. } => AgentRole::Planner,
                _ => AgentRole::PlanReviewer,
            };
            *corral_core::agent::container_host_mut(&mut task, container).unwrap() =
                Some(Box::new(AgentRunState::new(AgentBinding {
                    role,
                    store: "fixture".into(),
                    project: "app".into(),
                    generation: 0,
                    health: HostHealth::Fenced,
                    callback: None,
                    closing: true,
                })));
            recipient(container).state_mut(&mut task).unwrap().settled = None;
        }
        store.save(&task).unwrap();
        let mut expected = store.get(&id).unwrap();
        for container in &targets {
            recipient(container)
                .state_mut(&mut expected)
                .unwrap()
                .settled = Some(at);
        }
        expected.updated = store.now();
        let before = work::snapshot();
        let hot = HotScope::enter();
        let head = store
            .update_head(&id, |head| {
                for container in &targets {
                    assert!(head.settle_native_inbox(container, at)?);
                    assert!(!head.settle_native_inbox(container, at)?);
                    assert_eq!(
                        head.inbox_head(&recipient(container))?.unwrap().settled,
                        Some(at)
                    );
                }
                Ok(())
            })
            .unwrap();
        let delta = work::snapshot().since(&before);
        drop(hot);
        delta.check_identity().unwrap();
        assert_eq!(delta.cells(Counter::FullTaskHydrations).total, 0);
        costs.push(Counter::ALL.map(|counter| delta.cells(counter).unattributed));
        assert_eq!(store.get(&id).unwrap(), expected);
        assert_eq!(
            head.summary,
            corral_core::HistorySummary::of(&expected).unwrap()
        );
        let pending = head.unfinished_native_containers().unwrap();
        assert!(targets.iter().all(|container| !pending.contains(container)));
        let before = store.get(&id).unwrap();
        let mut foreign = targets[0].clone();
        if let ContainerRef::Worker { task, .. } = &mut foreign {
            *task = "T-999".into();
        }
        assert!(
            store
                .update_head(&id, |head| {
                    head.settle_native_inbox(&foreign, at)?;
                    Ok(())
                })
                .is_err()
        );
        assert_eq!(store.get(&id).unwrap(), before);
    }
    assert_eq!(
        costs[0], costs[1],
        "every named settlement counter independent of retained history"
    );
}
