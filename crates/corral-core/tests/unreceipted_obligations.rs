//! Attention needs receipt-free rows, not the wider stale-rendering reconciliation set.
mod store_support;
use corral_core::work::{self, Counter, HotScope};
use corral_core::{RunObligations, SlotWait, TaskRef};

#[test]
fn receipt_free_worker_and_reviewer_facets_ignore_stale_receipted_history_and_track_overlay() {
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
        let parent = task.runs.last_mut().unwrap();
        let run = parent.id.clone();
        let child = parent.review_runs.last_mut().unwrap();
        let review = child.id.clone();
        child.obligations.slot_wait = Some(SlotWait {
            since: store_support::at(1),
            on: vec!["T-999".into()],
        });
        let expected = task.unreceipted_obligations(&run).unwrap();
        let child_expected = task.unreceipted_review_obligations(&run, &review).unwrap();
        for rows in [&expected, &child_expected] {
            assert_eq!(rows.waits.len(), 1);
            assert_eq!(rows.decisions.len(), 1);
            assert!(rows.waits[0].receipt.is_none() && rows.decisions[0].receipt.is_none());
        }
        store.fixture_put_task(&task).unwrap();
        let head = store.head(&task.id).unwrap();
        // The old open facet MUST retain receipt-bearing rows needing render reconciliation.
        assert!(head.open_obligations(&run).unwrap().waits.len() > expected.waits.len());
        assert!(
            head.open_review_obligations(&run, &review)
                .unwrap()
                .waits
                .len()
                > child_expected.waits.len()
        );
        let hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(head.unreceipted_obligations(&run).unwrap(), expected);
        assert_eq!(
            head.unreceipted_review_obligations(&run, &review).unwrap(),
            child_expected
        );
        assert_eq!(
            head.unreceipted_obligations("missing").unwrap(),
            RunObligations::default()
        );
        assert_eq!(
            head.unreceipted_review_obligations(&run, "missing")
                .unwrap(),
            RunObligations::default()
        );
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|counter| after.total(counter) - before.total(counter)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
        drop(hot);
        let mut receipt = task.runs.last().unwrap().obligations.waits[0]
            .receipt
            .clone()
            .unwrap();
        receipt.id = "new-receipt".into();
        store
            .update_head(&task.id, |head| {
                assert_eq!(TaskRef::unreceipted_obligations(head, &run)?, expected);
                head.edit_wait(&run, &expected.waits[0].request.id, |wait| {
                    wait.receipt = Some(receipt);
                    Ok(())
                })?;
                assert!(
                    TaskRef::unreceipted_obligations(head, &run)?
                        .waits
                        .is_empty()
                );
                assert_eq!(
                    TaskRef::unreceipted_review_obligations(head, &run, &review)?,
                    child_expected
                );
                Ok(())
            })
            .unwrap();
        let changed = store.head(&task.id).unwrap();
        assert!(
            changed
                .unreceipted_obligations(&run)
                .unwrap()
                .waits
                .is_empty()
        );
        assert!(
            changed
                .open_obligations(&run)
                .unwrap()
                .waits
                .iter()
                .any(|wait| wait.request.id == expected.waits[0].request.id)
        );
    }
    assert_eq!(measured[0], measured[1]);
}
