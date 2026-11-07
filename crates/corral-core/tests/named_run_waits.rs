//! Historical wait mutation is explicit; the original edit_wait current-run guard is retained.
mod store_support;
use corral_core::work::{self, Counter, HotScope};
use corral_core::{
    ReceiptOutcome, ResumeDelivery, ResumeReceipt, TaskMut, TaskRef, WaitRecord, WaitState,
};

fn settle(wait: &mut WaitRecord, run: &str) {
    wait.observation.state = WaitState::Lost;
    wait.observation.at = store_support::at(100_000);
    wait.observation.detail = "lost".into();
    wait.receipt = Some(ResumeReceipt {
        version: 1,
        id: "job-receipt".into(),
        run: run.into(),
        outcome: ReceiptOutcome::Lost,
        delivery: ResumeDelivery::Acknowledged,
        observed_at: store_support::at(100_000),
        answer: None,
        data: serde_json::json!({"via":"job_monitor"}),
    });
}

#[test]
fn explicit_named_wait_edits_preserve_full_parity_facts_summary_and_current_guard_at_constant_work()
{
    for historical in [false, true] {
        let mut measured = Vec::new();
        for scale in [1, 10] {
            let (_dir, store) = store_support::store();
            store.fixture_pin_now(store_support::at(100_000)).unwrap();
            let task = store_support::rich(
                &store,
                2 * scale + 1,
                2 * scale + 1,
                2 * scale + 1,
                2 * scale,
            );
            let n = if historical { 0 } else { task.runs.len() - 1 };
            let run = task.runs[n].id.clone();
            let wait = task.runs[n]
                .obligations
                .waits
                .last()
                .unwrap()
                .request
                .id
                .clone();
            let mut expected = task.clone();
            if historical {
                assert!(TaskMut::edit_wait(&mut expected, &run, &wait, |_| Ok(())).is_err());
                assert!(
                    store
                        .update_head(&task.id, |head| head.edit_wait(&run, &wait, |_| Ok(())))
                        .is_err()
                );
            }
            TaskMut::edit_run_wait(&mut expected, &run, &wait, |wait| {
                settle(wait, &run);
                Ok(())
            })
            .unwrap();
            expected.updated = store.now();
            let hot = HotScope::enter();
            let before = work::snapshot();
            let head = store
                .update_head(&task.id, |head| {
                    TaskMut::edit_run_wait(head, &run, &wait, |wait| {
                        settle(wait, &run);
                        Ok(())
                    })?;
                    assert_eq!(
                        head.wait(&run, &wait)?.unwrap().observation.state,
                        WaitState::Lost
                    );
                    Ok(())
                })
                .unwrap();
            let after = work::snapshot();
            measured.push(Counter::ALL.map(|c| after.total(c) - before.total(c)));
            assert_eq!(
                after.total(Counter::FullTaskHydrations)
                    - before.total(Counter::FullTaskHydrations),
                0
            );
            drop(hot);
            assert_eq!(store.get(&task.id).unwrap(), expected);
            assert_eq!(
                head.summary,
                corral_core::HistorySummary::of_record(&store.task_record(&task.id).unwrap())
                    .unwrap()
            );
            let before = store.get(&task.id).unwrap();
            assert!(
                store
                    .update_head(&task.id, |head| head.edit_run_wait(&run, &wait, |wait| {
                        wait.request.id = "wrong".into();
                        Ok(())
                    }))
                    .is_err()
            );
            assert_eq!(store.get(&task.id).unwrap(), before);
        }
        assert_eq!(measured[0], measured[1], "historical={historical}");
    }
}
