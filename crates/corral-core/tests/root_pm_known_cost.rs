//! T-625: root AgentPm remains canonical, with exact visible read/write cost.
mod store_support;
use corral_core::{
    Store,
    agent::{AgentPm, AgentRunState, pm_state},
    work::{self, Counter, HotScope, WorkSnapshot},
};
use serde_json::json;

fn check(delta: &WorkSnapshot, load_bytes: Option<u64>, save_bytes: Option<u64>) {
    delta.check_identity().unwrap();
    for (site, bytes) in [("root_pm_load", load_bytes), ("root_pm_save", save_bytes)] {
        assert_eq!(
            delta.followup_calls("T-625", site),
            u64::from(bytes.is_some())
        );
        for counter in Counter::ALL {
            let expected = match (bytes, counter) {
                (Some(n), Counter::DecodedDocBytes) => n,
                (Some(n), Counter::RecordDecodes) => u64::from(n != 0),
                (Some(_), Counter::ViewsOpened) if site == "root_pm_load" => 1,
                _ => 0,
            };
            let cells = delta.cells(counter);
            assert_eq!(
                cells
                    .followup
                    .get(&("T-625".into(), site.into()))
                    .copied()
                    .unwrap_or(0),
                expected,
                "{site} {}",
                counter.name()
            );
            assert_eq!(cells.unattributed, 0, "{}", counter.name());
        }
    }
}

fn size(store: &Store) -> u64 {
    serde_json::to_vec(
        &store
            .record_row("agent-pm", "current")
            .unwrap()
            .unwrap()
            .doc,
    )
    .unwrap()
    .len() as u64
}

#[test]
fn pm_load_and_mutation_preserve_full_state_and_exact_named_document_cost_at_growth() {
    for scale in [1, 10] {
        let (_dir, store) = store_support::store();
        let before = work::snapshot();
        let hot = HotScope::enter();
        assert!(pm_state::load(&store).unwrap().is_none());
        let delta = work::snapshot().since(&before);
        drop(hot);
        check(&delta, Some(0), None);

        let mut state = AgentRunState::new(
            serde_json::from_value(json!({"role":"pm",
            "store":"fixture", "project":"app", "generation":2*scale}))
            .unwrap(),
        );
        // Repeated pre-hello generations are legitimate retained PM launch history.
        for n in 1..=2 * scale {
            state.generations.push(
                serde_json::from_value(json!({"n":n, "pane":"pm-pane",
                "bind_nonce_sha256":"0".repeat(64), "created":store_support::at(n as i64)}))
                .unwrap(),
            );
        }
        let original = AgentPm::new("pm-pane".into(), state);
        pm_state::update(&store, |pm| {
            *pm = Some(original.clone());
            Ok(((), true))
        })
        .unwrap();
        let bytes = size(&store);
        let before = work::snapshot();
        let hot = HotScope::enter();
        assert_eq!(pm_state::load(&store).unwrap().as_ref(), Some(&original));
        let delta = work::snapshot().since(&before);
        drop(hot);
        check(&delta, Some(bytes), None);

        let before = work::snapshot();
        let hot = HotScope::enter();
        pm_state::update(&store, |pm| {
            let pm = pm.as_mut().unwrap();
            assert_eq!(pm.state, original.state);
            pm.adopted_pane = Some(pm.pane.clone());
            Ok(((), true))
        })
        .unwrap();
        let delta = work::snapshot().since(&before);
        drop(hot);
        check(&delta, Some(bytes), Some(bytes));
        let mut expected = original;
        expected.adopted_pane = Some(expected.pane.clone());
        assert_eq!(pm_state::load(&store).unwrap(), Some(expected));
    }
}
