//! Selected complete Task identity, not ordinary PM discovery or policy, is T-678.
use super::*;
use crate::agent::native_test_support::{self as native, data};
use corral_core::{
    backend::{TaskNum, TaskParts},
    work::{self, Counter, HotScope},
};

#[test]
fn selected_identity_keeps_exact_main_bytes_with_independent_canonical_cost_at_growth() {
    for scale in [1, 10] {
        let root = tempfile::tempdir().unwrap();
        let store = corral_core::Store::init(root.path()).unwrap();
        store.fixture_pin_now(data::at(10)).unwrap();
        let task = data::rich(&store, 2 * scale, 2 * scale, 2 * scale, 2 * scale);
        let mut original = None;
        for attention in [false, true] {
            if attention {
                store
                    .update_attention(&task.id, |ledger| {
                        ledger.overflow_count = Some(99);
                        Ok(())
                    })
                    .unwrap();
            }
            let mut full = store.get(&task.id).unwrap();
            full.attention = corral_core::AttentionLedger::default();
            let expected_bytes = serde_json::to_vec(&full).unwrap();
            if let Some(original) = &original {
                assert_eq!(&expected_bytes, original);
            } else {
                original = Some(expected_bytes.clone());
            }
            let ctx = corral_ops_context(store.read_only_snapshot().unwrap());
            let doc = ctx
                .store
                .view()
                .unwrap()
                .task(TaskNum(1), &TaskParts::All)
                .unwrap()
                .unwrap();
            let mut expected = native::full_read_cost(&doc);
            expected.insert(Counter::ViewsOpened, 0); // discovery already pinned the one view
            let hot = HotScope::enter();
            let before = work::snapshot();
            let bytes = selected_task_identity(&ctx, &task.id).unwrap();
            let report = work::snapshot().since(&before);
            drop(hot);
            assert_eq!(bytes, expected_bytes);
            report.check_identity().unwrap();
            assert_eq!(report.followup_calls("T-678", "pm_action_task_identity"), 1);
            assert_eq!(report.followup_calls.len(), 1);
            for counter in Counter::ALL {
                let cells = report.cells(counter);
                assert_eq!(
                    cells
                        .followup
                        .get(&("T-678".into(), "pm_action_task_identity".into()))
                        .copied()
                        .unwrap_or(0),
                    expected[&counter],
                    "{counter:?}"
                );
                assert_eq!(cells.unattributed, 0, "{counter:?}");
            }
        }
        store
            .update(&task.id, |task| {
                task.title.push_str("changed content");
                Ok(())
            })
            .unwrap();
        let ctx = corral_ops_context(store.read_only_snapshot().unwrap());
        assert_ne!(
            selected_task_identity(&ctx, &task.id).unwrap(),
            original.unwrap()
        );
    }
}

fn corral_ops_context(store: corral_core::Store) -> Ctx {
    crate::resources::FixedResourceProbe::ctx(store)
}
