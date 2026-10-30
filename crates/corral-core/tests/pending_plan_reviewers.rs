//! Attention must retain the last pending reviewer of older plans, not scan plan history.
mod store_support;
use corral_core::work::{self, Counter, HotScope};
use corral_core::{TaskRef, Verdict};

#[test]
fn latest_pending_reviewers_preserve_full_semantics_with_constant_work() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let (_dir, store) = store_support::store();
        store.fixture_pin_now(store_support::at(0)).unwrap();
        let mut task = store_support::rich(&store, 1, 1, 3 * scale, 2 * scale);
        for plan in &mut task.plans {
            for review in &mut plan.reviews {
                review.verdict = Some(Verdict::Approve);
            }
        }
        // An old plan still owns delivery. A different plan has an old pending review
        // followed by a closed replacement: the old one must NOT reappear.
        task.plans[0].reviews.last_mut().unwrap().verdict = None;
        let mut replacement = task.plans[1].reviews[0].clone();
        let mut replaced = replacement.clone();
        replaced.verdict = None;
        task.plans[1].reviews = (0..2 * scale)
            .map(|n| {
                let mut old = replaced.clone();
                old.id = format!("replaced-{n:06}");
                old
            })
            .collect();
        replacement.id = "replacement-closed".into();
        task.plans[1].reviews.push(replacement);
        task.plans
            .last_mut()
            .unwrap()
            .reviews
            .last_mut()
            .unwrap()
            .verdict = None;
        store.fixture_put_task(&task).unwrap();
        let expected = task.pending_plan_reviewers().unwrap();
        assert_eq!(expected.len(), 2);
        assert_eq!(expected[0].0, task.plans[0].id);
        assert_eq!(expected[1].0, task.plans.last().unwrap().id);
        let head = store.head(&task.id).unwrap();
        {
            let _hot = HotScope::enter();
            let before = work::snapshot();
            assert_eq!(head.pending_plan_reviewers().unwrap(), expected);
            let after = work::snapshot();
            measured.push(Counter::ALL.map(|counter| after.total(counter) - before.total(counter)));
            assert_eq!(
                after.total(Counter::FullTaskHydrations)
                    - before.total(Counter::FullTaskHydrations),
                0
            );
        }
        let current_plan = expected[1].0.clone();
        let review = expected[1].1.id.clone();
        store
            .update_head(&task.id, |head| {
                head.edit_plan_review(&current_plan, &review, |review| {
                    review.verdict = Some(Verdict::Approve);
                    Ok(())
                })?;
                // The same indexed projection must honor the mutation's queued row overlay.
                assert_eq!(TaskRef::pending_plan_reviewers(head)?.len(), 1);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            store
                .head(&task.id)
                .unwrap()
                .pending_plan_reviewers()
                .unwrap()
                .len(),
            1
        );
    }
    assert_eq!(measured[0], measured[1]);
}
