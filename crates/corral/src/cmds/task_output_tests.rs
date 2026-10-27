use super::*;
use corral_core::work::{self, Counter};

#[path = "../../../corral-core/tests/store_support/mod.rs"]
mod support;

#[test]
fn list_open_has_identical_counters_with_tenfold_terminal_and_nested_history() {
    let mut reports = Vec::new();
    for factor in [1, 10] {
        // Given the same open task and tenfold unrelated and nested history.
        let (_dir, store) = support::store();
        let mut task = store.create("Open", None, Status::Running, "Spec").unwrap();
        support::grow(&mut task, 3 * factor, 3 * factor, 3 * factor, 3 * factor);
        task.created = support::at(0);
        task.updated = support::at(0);
        store.fixture_put_task(&task).unwrap();
        for _ in 0..10 * factor {
            store.create("Old", None, Status::Done, "").unwrap();
        }
        let ctx = Ctx::from_store(store);
        let before = work::snapshot();
        // When the real open-list path prints JSON and configured card columns.
        list(&ctx, None, None, false, true, &[], None).unwrap();
        list(
            &ctx,
            None,
            None,
            false,
            false,
            &[
                "cost".into(),
                "tokens".into(),
                "artifacts".into(),
                "lines".into(),
                "depends_on".into(),
            ],
            None,
        )
        .unwrap();
        let report = work::snapshot().since(&before);
        // Then terminal/history growth is absent from every unattributed counter.
        assert_eq!(report.unattributed_hydrations(), 0);
        report.check_identity().unwrap();
        reports.push(report);
    }
    for counter in Counter::ALL {
        assert_eq!(
            reports[0].get(counter),
            reports[1].get(counter),
            "{counter:?}"
        );
    }
}

#[test]
fn list_summary_equals_full_oracle_and_show_retains_history() {
    // Given a rich task with spec body and nested history.
    let (_dir, store) = support::store();
    let task = support::rich(&store, 3, 4, 3, 4);
    let head = store.head(&task.id).unwrap();
    // When summary and full show JSON are produced.
    let summary = task_head_json(&head).unwrap();
    let full = task_json(&task, true);
    // Then list uses the established summary shape, while show remains full.
    assert_eq!(summary, task_json(&task, false));
    for field in HISTORY {
        assert!(summary.get(field).is_none(), "{field}");
    }
    assert!(summary.get("path").is_none());
    assert_eq!(summary["current_run"]["id"], task.current_run().unwrap().id);
    assert_eq!(full["runs"].as_array().unwrap().len(), task.runs.len());
    assert_eq!(full["body"], task.body);
    assert_eq!(
        full["comments"].as_array().unwrap().len(),
        task.comments.len()
    );
}
