//! T-626 is only the canonical retained-vector mutation, never bounded admission/replay/postwork.
use super::{
    native_test_support::{self as native, data},
    test_support as ts,
};
use corral_core::{
    agent::{self, AgentRole, ContainerRef, paths},
    backend::{TaskNum, TaskParts},
    work::{self, Counter, HotScope},
};

#[test]
fn native_event_has_exact_canonical_t626_cost_and_constant_unattributed_replay_work() {
    let mut costs = Vec::new();
    for scale in [1, 10] {
        corral_core::clock::pin_thread(Some(data::at(100_000)));
        let (dir, store) = data::store();
        store.fixture_pin_now(data::at(100_000)).unwrap();
        let mut task = data::rich(
            &store,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale,
        );
        task.project = Some("corral".into());
        let run = task.runs.last_mut().unwrap();
        run.harness = Some(corral_core::PI_NATIVE_HARNESS.into());
        let mut state = native::host(AgentRole::Worker, 2 * scale);
        state.binding.store = paths::store_id(&store);
        state.binding.project = "corral".into();
        run.agent_host = Some(Box::new(state));
        let run_id = run.id.clone();
        let fx = ts::Fx {
            _tmp: dir,
            ctx: crate::Ctx::from_store(store),
        };
        run.herdr = Some(corral_core::HerdrRef {
            workspace_id: "w1".into(),
            pane_id: "p1".into(),
            socket: Some(fx.ctx.herdr.socket_path().unwrap()),
        });
        fx.ctx.store.save(&task).unwrap();
        let target = super::BindTarget::Worker {
            task: task.id.clone(),
            run: run_id.clone(),
        };
        let before_doc = fx
            .ctx
            .store
            .view()
            .unwrap()
            .task(TaskNum(1), &TaskParts::All)
            .unwrap()
            .unwrap();
        let expected_open = native::full_read_cost(&before_doc);
        let before = work::snapshot();
        let hot = HotScope::enter();
        let host = ts::bind(&fx, &target, "p1", "fixed-event-session");
        let delta = work::snapshot().since(&before);
        drop(hot);
        delta.check_identity().unwrap();
        assert_eq!(delta.followup_calls("T-626", "native_generation_open"), 1);
        for counter in Counter::ALL {
            assert_eq!(
                delta
                    .cells(counter)
                    .followup
                    .get(&("T-626".into(), "native_generation_open".into()))
                    .copied()
                    .unwrap_or(0),
                expected_open[&counter],
                "generation {}",
                counter.name()
            );
        }
        assert_eq!(host.identity.generation, 1000);
        assert_eq!(delta.cells(Counter::FullTaskHydrations).unattributed, 0);
        costs.push(Counter::ALL.map(|counter| delta.cells(counter).unattributed));
        ts::committed(&ts::hello(&fx, &host));
        let doc = fx
            .ctx
            .store
            .view()
            .unwrap()
            .task(TaskNum(1), &TaskParts::All)
            .unwrap()
            .unwrap();
        let expected = native::full_read_cost(&doc);
        let input = ts::event_input(&host, 1, "turn_started", 1);
        let before = work::snapshot();
        let hot = HotScope::enter();
        let receipt = ts::committed(&ts::send(&fx, &input, "event"));
        let delta = work::snapshot().since(&before);
        drop(hot);
        delta.check_identity().unwrap();
        assert_eq!(delta.followup_calls("T-626", "native_event_retention"), 1);
        for counter in Counter::ALL {
            let cells = delta.cells(counter);
            assert_eq!(
                cells
                    .followup
                    .get(&("T-626".into(), "native_event_retention".into()))
                    .copied()
                    .unwrap_or(0),
                expected[&counter],
                "{}",
                counter.name()
            );
        }
        assert_eq!(delta.cells(Counter::FullTaskHydrations).unattributed, 0);
        costs.push(Counter::ALL.map(|counter| delta.cells(counter).unattributed));
        let container = ContainerRef::Worker {
            task: task.id.clone(),
            run: run_id,
        };
        let turn = fx
            .ctx
            .store
            .head(&task.id)
            .unwrap()
            .agent_turn(&container, host.identity.generation, 1)
            .unwrap()
            .unwrap();
        assert!(turn.started.is_some());
        assert!(turn.ended.is_none());
        let before = work::snapshot();
        let hot = HotScope::enter();
        let replay = ts::send(&fx, &input, "event");
        let delta = work::snapshot().since(&before);
        drop(hot);
        delta.check_identity().unwrap();
        assert_eq!(delta.followup_calls("T-626", "native_event_retention"), 0);
        assert!(
            matches!(replay, super::call::CallResponse::Receipt { receipt:stored, replayed:true } if stored == receipt)
        );
        assert_eq!(delta.cells(Counter::FullTaskHydrations).total, 0);
        costs.push(Counter::ALL.map(|counter| delta.cells(counter).unattributed));
        assert_eq!(
            ts::state(&fx, &container)
                .turns
                .iter()
                .filter(|turn| turn.generation == host.identity.generation && turn.seq == 1)
                .count(),
            1
        );
        assert!(
            agent::container_closure(&fx.ctx.store.get(&task.id).unwrap(), &container).is_none()
        );
    }
    assert_eq!(
        costs[0], costs[3],
        "generation opening's bounded remainder must not follow retained history"
    );
    assert_eq!(
        costs[1], costs[4],
        "event's bounded remainder must not follow retained task/native history"
    );
    assert_eq!(
        costs[2], costs[5],
        "all replay counters must be history-independent"
    );
}
