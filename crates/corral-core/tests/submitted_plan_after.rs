//! Reservation outcome keeps first-append semantics, not newest matching-plan semantics.
mod store_support;
use corral_core::{
    TaskMut, TaskRef,
    work::{self, Counter, HotScope},
};

#[test]
fn submitted_plan_after_uses_run_index_and_cursor_with_full_overlay_growth_parity() {
    let mut costs = Vec::new();
    for scale in [1, 10] {
        let (_dir, store) = store_support::store();
        store.fixture_pin_now(store_support::at(100_000)).unwrap();
        let mut task = store_support::rich(
            &store,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale,
        );
        for plan in &mut task.plans {
            plan.run = Some("other".into());
        }
        task.plans[0].run = Some("planner-fixed".into());
        let before = task.plans.len();
        let mut first = task.plans.last().unwrap().clone();
        first.id = "mine-first".into();
        first.run = Some("planner-fixed".into());
        let mut unrelated = first.clone();
        unrelated.id = "unrelated-after".into();
        unrelated.run = Some("other".into());
        let mut last = first.clone();
        last.id = "mine-last".into();
        task.plans.extend([first.clone(), unrelated, last.clone()]);
        store.fixture_put_task(&task).unwrap();
        let full = store.get(&task.id).unwrap();
        let expected = TaskRef::submitted_plan_after(&full, "planner-fixed", before).unwrap();
        assert_eq!(
            expected,
            Some((first.id.clone(), first.artifact.sha256.clone()))
        );
        let head = store.head(&task.id).unwrap();
        let before_work = work::snapshot();
        let hot = HotScope::enter();
        assert_eq!(
            head.submitted_plan_after("planner-fixed", before).unwrap(),
            expected
        );
        assert_eq!(
            head.submitted_plan_after("planner-fixed", before + 1)
                .unwrap(),
            Some((last.id.clone(), last.artifact.sha256.clone()))
        );
        assert!(
            head.submitted_plan_after("planner-fixed", before + 3)
                .unwrap()
                .is_none()
        );
        let mut appended = first;
        appended.id = "appended".into();
        store
            .update_head(&task.id, |head| {
                head.push_plan(appended.clone())?;
                assert_eq!(
                    head.submitted_plan_after("planner-fixed", before + 3)?,
                    Some((appended.id.clone(), appended.artifact.sha256.clone()))
                );
                Ok(())
            })
            .unwrap();
        let delta = work::snapshot().since(&before_work);
        drop(hot);
        delta.check_identity().unwrap();
        assert_eq!(delta.cells(Counter::FullTaskHydrations).total, 0);
        costs.push(Counter::ALL.map(|counter| delta.cells(counter).unattributed));
        let full = store.get(&task.id).unwrap();
        assert_eq!(
            TaskRef::submitted_plan_after(&full, "planner-fixed", before + 3).unwrap(),
            Some((appended.id, appended.artifact.sha256))
        );
    }
    assert_eq!(
        costs[0], costs[1],
        "all submission lookup/append/overlay counters remain fixed"
    );
}
