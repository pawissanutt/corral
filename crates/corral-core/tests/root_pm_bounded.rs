//! The root PM hot path must not decode or rewrite retained child history.
mod store_support;
use corral_core::{
    Store,
    agent::{AgentPm, AgentRunState, NativeRecord, pm_state},
    work::{self, Counter, HotScope},
};
use serde_json::json;

fn measure<T>(f: impl FnOnce() -> T) -> (T, Vec<u64>) {
    let before = work::snapshot();
    let hot = HotScope::enter();
    let result = f();
    drop(hot);
    let delta = work::snapshot().since(&before);
    delta.check_identity().unwrap();
    assert!(delta.followup_calls.is_empty(), "{delta:?}");
    assert_eq!(delta.unattributed_hydrations(), 0);
    (
        result,
        Counter::ALL.into_iter().map(|c| delta.total(c)).collect(),
    )
}

#[test]
fn pm_selected_reads_and_patches_are_history_independent_and_preserve_unselected_children() {
    let mut costs = Vec::new();
    for scale in [2, 4] {
        let (dir, store) = store_support::store();
        let mut state = AgentRunState::new(
            serde_json::from_value(json!({
                "role":"pm", "store":"fixture", "project":"app", "generation":9
            }))
            .unwrap(),
        );
        for n in (1..=scale).chain([9]) {
            state.generations.push(
                serde_json::from_value(json!({
                    "n":n, "pane":"pm-pane", "bind_nonce_sha256":"0".repeat(64),
                    "created":store_support::at(1)
                }))
                .unwrap(),
            );
        }
        for n in 1..=scale {
            state.turns.push(
                serde_json::from_value(json!({
                    "generation":1,"seq":n,"origin":{"kind":"unknown"},
                    "ended":store_support::at(1)
                }))
                .unwrap(),
            );
        }
        state.turns.push(
            serde_json::from_value(json!({
                "generation":9,"seq":1,"origin":{"kind":"unknown"}
            }))
            .unwrap(),
        );
        let original = AgentPm::new("pm-pane".into(), state);
        // Exercise the actual v1 upgrade, not a second fixture-only encoder.
        store.save_record("agent-pm", "current", &original).unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(pm_state::load(&store).unwrap(), Some(original.clone()));
        let head = store.record_row("agent-pm", "current").unwrap().unwrap();
        assert_eq!(head.doc["layout"], 2);
        assert!(head.doc["pm"]["state"].get("generations").is_none());
        assert!(head.doc["pm"]["state"].get("turns").is_none());
        let old_generations = store.record_rows("agent-pm-generations").unwrap();
        let old_turns = store.record_rows("agent-pm-turns").unwrap();
        // Both reopened and warm reads must hit only the head + current generation.
        let reopened = Store::open(dir.path()).unwrap();
        for reader in [&store, &reopened] {
            let (pm, cost) = measure(|| pm_state::load_current(reader).unwrap().unwrap());
            assert_eq!(pm.state.generations.len(), 1);
            assert!(pm.state.turns.is_empty());
            assert_eq!(cost[Counter::RecordDecodes as usize], 2);
            costs.push(cost);
        }
        let (_, cost) = measure(|| {
            pm_state::update_selected(&store, &pm_state::Selection::current(), |pm| {
                let pm = pm.as_mut().unwrap();
                pm.state.generation_mut(9).unwrap().event_seq = 7;
                Ok(((), true))
            })
            .unwrap()
        });
        // Keep mutation cost separate from the two read costs.
        let mutation_cost = cost;
        let mut expected = original.clone();
        expected.state.generation_mut(9).unwrap().event_seq = 7;
        assert_eq!(pm_state::load(&store).unwrap(), Some(expected.clone()));
        let selection = pm_state::Selection::work();
        let (mut pm, work_cost) = measure(|| {
            pm_state::load_selected(&store, &selection)
                .unwrap()
                .unwrap()
        });
        assert_eq!(pm.state.turns.len(), 1);
        assert_eq!(pm.state.turns[0].generation, 9);
        pm.state.turns[0].ended = Some(store_support::at(2));
        let turn = NativeRecord::Turn(pm.state.turns[0].clone());
        pm_state::update_selected(&store, &selection, |pm| {
            pm.as_mut().unwrap().state.turns[0] =
                serde_json::from_value(serde_json::to_value(&turn).unwrap()).unwrap();
            Ok(((), true))
        })
        .unwrap();
        assert!(
            pm_state::load_selected(&store, &selection)
                .unwrap()
                .unwrap()
                .state
                .turns
                .is_empty()
        );
        expected.state.turns.last_mut().unwrap().ended = Some(store_support::at(2));
        assert_eq!(pm_state::load(&reopened).unwrap(), Some(expected));
        for row in old_generations {
            if row.doc["record"]["n"] != 9 {
                assert_eq!(store.record_row(&row.area, &row.key).unwrap().unwrap(), row);
            }
        }
        for row in old_turns {
            if row.doc["record"]["generation"] != 9 {
                assert_eq!(store.record_row(&row.area, &row.key).unwrap().unwrap(), row);
            }
        }
        println!(
            "PM scale={scale}: read={:?} mutation={mutation_cost:?} work={work_cost:?}",
            costs[0]
        );
        if scale == 2 {
            costs.extend([mutation_cost, work_cost]);
        } else {
            assert_eq!(mutation_cost, costs[2]);
            assert_eq!(work_cost, costs[3]);
        }
    }
    assert_eq!(costs[0], costs[1]);
    assert_eq!(costs[0], costs[4]);
    assert_eq!(costs[0], costs[5]);
}
