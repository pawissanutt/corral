//! T-651: settled operational results stay durable, but not in bounded attention/card reads.
mod store_support;
use corral_core::work::{self, Counter, HotScope};
use corral_core::{RunHead, Store, TaskRef, WaitHandle, WaitRecord, WaitState};
use store_support::at;

fn job(template: &WaitRecord, run: &str, n: usize, state: WaitState) -> WaitRecord {
    let mut wait = template.clone();
    wait.request.id = format!("job-{n:04}");
    wait.request.run = run.into();
    wait.request.condition = format!("job j{n:04} exits");
    wait.request.handle = WaitHandle::Channel {
        channel_id: format!("job:j{n:04}"),
        capability: Some("wait_v1".into()),
    };
    wait.receipt = None;
    wait.observation.state = state;
    wait
}

#[test]
fn settled_job_facets_keep_full_history_and_exact_warm_reopened_read_work() {
    let mut budgets = Vec::new();
    for scale in [1, 10] {
        let (root, store) = store_support::store();
        store.fixture_pin_now(at(10)).unwrap();
        let mut task = store_support::rich(&store, 2, 2, 2, 0);
        let parent = task.runs.last_mut().unwrap();
        let run = parent.id.clone();
        let template = parent.obligations.waits[0].clone();
        parent.obligations.waits.clear();
        let child = parent.review_runs.last_mut().unwrap();
        let review = child.id.clone();
        child.obligations.waits.clear();
        for n in 0..3 * scale {
            let state = [WaitState::Ready, WaitState::Failed, WaitState::Expired][n % 3];
            parent
                .obligations
                .waits
                .push(job(&template, &run, n, state));
            parent
                .review_runs
                .last_mut()
                .unwrap()
                .obligations
                .waits
                .push(job(&template, &review, n, state));
        }
        store.fixture_put_task(&task).unwrap();
        let expected = task.attention_obligations(&run).unwrap();
        let expected_child = task.attention_review_obligations(&run, &review).unwrap();
        assert!(expected.waits.is_empty() && expected_child.waits.is_empty());
        for reopened in [false, true] {
            let store = if reopened {
                Store::open(root.path()).unwrap()
            } else {
                store.clone()
            };
            let head = store.head(&task.id).unwrap();
            // Existing exact APIs must not have their receipt-free semantics weakened.
            assert_eq!(
                head.unreceipted_obligations(&run).unwrap().waits.len(),
                3 * scale
            );
            assert_eq!(
                head.unreceipted_review_obligations(&run, &review)
                    .unwrap()
                    .waits
                    .len(),
                3 * scale
            );
            let facts = head.run_head(&run).unwrap().unwrap().facts;
            assert_eq!(facts.settled_job_exits, (3 * scale) as u64);
            assert_eq!(
                head.review_child(&run, &review)
                    .unwrap()
                    .unwrap()
                    .facts
                    .settled_job_exits,
                (3 * scale) as u64
            );
            let hot = HotScope::enter();
            let before = work::snapshot();
            assert_eq!(head.attention_obligations(&run).unwrap(), expected);
            assert_eq!(
                head.attention_review_obligations(&run, &review).unwrap(),
                expected_child
            );
            assert!(head.card_obligations(&run).unwrap().waits.is_empty());
            let delta = work::snapshot().since(&before);
            drop(hot);
            delta.check_identity().unwrap();
            assert_eq!(delta.total(Counter::FullTaskHydrations), 0);
            budgets.push(delta);
        }
        store
            .update_head(&task.id, |head| {
                head.edit_wait(&run, "job-0000", |wait| {
                    wait.observation.state = WaitState::Pending;
                    Ok(())
                })?;
                assert_eq!(TaskRef::attention_obligations(head, &run)?.waits.len(), 1);
                assert_eq!(
                    head.run_head(&run)?.unwrap().facts.settled_job_exits,
                    (3 * scale - 1) as u64
                );
                head.push_wait(&run, job(&template, &run, 100, WaitState::Failed))?;
                assert_eq!(
                    head.run_head(&run)?.unwrap().facts.settled_job_exits,
                    (3 * scale) as u64
                );
                Ok(())
            })
            .unwrap();
        let full = store.get(&task.id).unwrap();
        let actual = store.head(&task.id).unwrap();
        assert_eq!(
            actual.attention_obligations(&run).unwrap(),
            full.attention_obligations(&run).unwrap()
        );
        assert_eq!(
            actual
                .run_head(&run)
                .unwrap()
                .unwrap()
                .facts
                .settled_job_exits,
            RunHead::of(full.current_run().unwrap())
                .unwrap()
                .facts
                .settled_job_exits
        );
        assert_eq!(actual.card_obligations(&run).unwrap().waits.len(), 1);
        // Historical explicit wait edits update the same summary without reopening a current run.
        let old_run = full.runs[0].id.clone();
        let old_wait = full.runs[0].obligations.waits[0].request.id.clone();
        store
            .update_head(&task.id, |head| {
                head.edit_run_wait(&old_run, &old_wait, |wait| {
                    *wait = job(&template, &old_run, 101, WaitState::Expired);
                    wait.request.id = old_wait.clone();
                    Ok(())
                })
            })
            .unwrap();
        let full = store.get(&task.id).unwrap();
        assert_eq!(
            store
                .head(&task.id)
                .unwrap()
                .run_head(&old_run)
                .unwrap()
                .unwrap()
                .facts
                .settled_job_exits,
            RunHead::of(&full.runs[0]).unwrap().facts.settled_job_exits
        );
    }
    assert_eq!(budgets[0], budgets[2]);
    assert_eq!(budgets[1], budgets[3]);
}
