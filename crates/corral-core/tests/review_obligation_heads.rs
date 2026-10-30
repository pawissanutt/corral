//! Reviewer attention reads its own active obligations, not its worker or settled history.
mod store_support;
use corral_core::work::{self, Counter, HotScope};
use corral_core::{RunObligations, SlotWait, TaskRef};

fn settle(obligations: &mut RunObligations) {
    for wait in &mut obligations.waits {
        if let Some(receipt) = &wait.receipt {
            wait.observation = corral_core::receipt_observation(receipt, wait.observation.at);
        }
    }
}

#[test]
fn reviewer_obligations_are_scoped_and_output_sized_at_tenfold_history() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let (_dir, store) = store_support::store();
        let mut task = store_support::rich(
            &store,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale,
        );
        for parent in &mut task.runs {
            settle(&mut parent.obligations);
            for child in &mut parent.review_runs {
                settle(&mut child.obligations);
            }
        }
        let parent = task.runs.last_mut().unwrap();
        let run = parent.id.clone();
        let child = parent.review_runs.last_mut().unwrap();
        let review = child.id.clone();
        // Keep one unfinished rendering with a receipt as well as the genuinely pending wait.
        // `open` is deliberately broader than `receipt.is_none()`.
        child.obligations.waits[0].observation.detail = "stale rendering".into();
        child.obligations.slot_wait = Some(SlotWait {
            since: store_support::at(1),
            on: vec!["T-999".into()],
        });
        let expected = task.open_review_obligations(&run, &review).unwrap();
        assert_eq!(expected.waits.len(), 2);
        assert_eq!(expected.decisions.len(), 1);
        assert!(expected.waits.iter().any(|wait| wait.receipt.is_some()));
        assert!(expected.waits.iter().all(|wait| wait.request.run == review));
        assert!(
            expected
                .decisions
                .iter()
                .all(|decision| decision.request.run == review)
        );
        store.fixture_put_task(&task).unwrap();
        let head = store.head(&task.id).unwrap();
        {
            let _hot = HotScope::enter();
            let before = work::snapshot();
            assert_eq!(
                head.open_review_obligations(&run, &review).unwrap(),
                expected
            );
            assert_eq!(
                head.open_review_obligations(&run, "missing").unwrap(),
                RunObligations::default()
            );
            assert_eq!(
                head.open_review_obligations("missing", &review).unwrap(),
                RunObligations::default()
            );
            let after = work::snapshot();
            measured.push(Counter::ALL.map(|counter| after.total(counter) - before.total(counter)));
            assert_eq!(
                after.total(Counter::FullTaskHydrations)
                    - before.total(Counter::FullTaskHydrations),
                0
            );
        }
        let old = task.runs.last().unwrap().review_runs[0].id.clone();
        assert_eq!(
            head.open_review_obligations(&run, &old).unwrap(),
            task.open_review_obligations(&run, &old).unwrap()
        );
        store
            .update_head(&task.id, |head| {
                assert_eq!(
                    TaskRef::open_review_obligations(head, &run, &review)?,
                    expected
                );
                Ok(())
            })
            .unwrap();
    }
    assert_eq!(measured[0], measured[1]);
}
