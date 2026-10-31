//! Ordinary catalog artifact output stays headed; historical query expansion is exact T-672.
mod store_support;
use corral_core::{
    Store,
    work::{self, Counter, HotScope},
};

#[test]
fn headed_catalog_artifact_output_reads_only_its_actual_rows() {
    let mut costs = Vec::new();
    for scale in [1, 10] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        store.fixture_pin_now(store_support::at(10)).unwrap();
        let mut task = store_support::rich(&store, 2 * scale, 2 * scale, 2 * scale, 2 * scale);
        task.artifacts.truncate(1);
        store.fixture_put_task(&task).unwrap();
        let hot = HotScope::enter();
        let before = work::snapshot();
        let head = store.head(&task.id).unwrap();
        assert_eq!(store.artifact_rows(&head.id, None).unwrap(), task.artifacts);
        let report = work::snapshot().since(&before);
        drop(hot);
        report.check_identity().unwrap();
        assert_eq!(report.unattributed_hydrations(), 0);
        assert_eq!(report.followup_calls.len(), 0);
        assert_eq!(report.get(Counter::HistoryRowDecodes), 1);
        costs.push(report);
    }
    for counter in Counter::ALL {
        assert_eq!(
            costs[0].total(counter),
            costs[1].total(counter),
            "{counter:?}"
        );
    }
}
