//! Native sync attention/hold release reads actual worker metadata, not retained vectors.
use super::sync::*;
use crate::agent::native_test_support::{self as native, data};
use corral_core::{
    RunOutcome, Status,
    agent::{AgentRole, HostHealth},
    work::{self, Counter, HotScope},
};
use serde_json::json;

#[test]
fn worker_attention_and_hold_release_match_full_evidence_with_exact_history_growth() {
    let mut costs = Vec::new();
    for scale in [1, 10] {
        for case in ["lost", "auth", "recovered", "closed"] {
            let (_dir, store) = data::store();
            store.fixture_pin_now(data::at(100_000)).unwrap();
            let mut task = data::rich(
                &store,
                2 * scale + 1,
                2 * scale + 1,
                2 * scale + 1,
                2 * scale,
            );
            task.status = Status::NeedsMe;
            let run = task.runs.last_mut().unwrap();
            run.blocked_excerpt = Some(format!("{LOST_PREFIX}fixture"));
            let mut host = native::host(AgentRole::Worker, 2 * scale);
            if case == "lost" {
                host.binding.health = HostHealth::Lost;
            }
            if case == "auth" {
                host.turns.push(
                    serde_json::from_value(json!({"generation":999, "seq":1,
                    "origin":{"kind":"unknown"}, "started":data::at(2), "ended":data::at(3),
                    "class":"provider_failed", "provider_error":"auth"}))
                    .unwrap(),
                );
            }
            if case == "closed" {
                run.outcome = Some(RunOutcome::Done);
            }
            run.agent_host = Some(Box::new(host));
            store.save(&task).unwrap();
            let full = store.get(&task.id).unwrap();
            let expected = attention(&full);
            let should_release = expected.is_none()
                && full
                    .current_run()
                    .unwrap()
                    .obligations
                    .pending_decision()
                    .is_none();
            let ctx = crate::Ctx::from_store(store);
            let head = ctx.store.head(&task.id).unwrap();
            let before = work::snapshot();
            let hot = HotScope::enter();
            assert_eq!(attention_ref(&head).unwrap(), expected, "{case}");
            assert_eq!(
                settle_lost(&ctx, &task.id).unwrap(),
                should_release,
                "{case}"
            );
            let delta = work::snapshot().since(&before);
            drop(hot);
            delta.check_identity().unwrap();
            assert_eq!(delta.cells(Counter::FullTaskHydrations).total, 0);
            costs.push(Counter::ALL.map(|counter| delta.cells(counter).unattributed));
            let after = ctx.store.get(&task.id).unwrap();
            assert_eq!(
                after.status,
                if should_release {
                    Status::Running
                } else {
                    Status::NeedsMe
                }
            );
            assert_eq!(
                after.current_run().unwrap().blocked_excerpt.is_none(),
                should_release
            );
        }
    }
    for i in 0..4 {
        assert_eq!(
            costs[i],
            costs[i + 4],
            "every attention/hold-release counter at case {i}"
        );
    }
}
