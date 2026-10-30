use super::*;
use crate::agent::test_support as ts;
use corral_core::RunOutcome;
use corral_core::work::{self, Counter, HotScope};

#[test]
fn owed_event_replay_is_keyed_and_history_independent() {
    struct Unpin;
    impl Drop for Unpin {
        fn drop(&mut self) {
            corral_core::clock::pin_thread(None);
        }
    }
    let at = "2026-01-01T00:00:00Z".parse().unwrap();
    corral_core::clock::pin_thread(Some(at));
    let _clock = Unpin;
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let fx = ts::fx();
        let id = ts::worker_task(&fx, "p1");
        let host = ts::bind(
            &fx,
            &super::super::BindTarget::Worker {
                task: id.clone(),
                run: "R1".into(),
            },
            "p1",
            "worker",
        );
        let mut owed = ts::committed(&ts::hello(&fx, &host));
        owed.request_id = "owed-event".into();
        owed.op = "progress".into();
        owed.result = json!({"event":{"kind":"fixture_effect","detail":{"value":"kept"}}});
        owed.post_pending = vec!["effect:event".into()];
        let mut task = fx.ctx.store.get(&id).unwrap();
        let current = task.current_run().unwrap().clone();
        for n in 0..2 * scale {
            let mut old = current.clone();
            old.id = format!("old-{n:04}");
            old.outcome = Some(RunOutcome::Done);
            old.ended = Some(at);
            old.inbox.settled = Some(at);
            old.agent_host = None;
            task.runs.insert(0, old);
        }
        task.current_run_mut()
            .unwrap()
            .agent_host
            .as_mut()
            .unwrap()
            .receipts
            .push(owed.clone());
        fx.ctx.store.fixture_put_task(&task).unwrap();
        let container = ContainerRef::Worker {
            task: id.clone(),
            run: "R1".into(),
        };
        let origin = RequestOrigin {
            request_id: owed.request_id.clone(),
            op: owed.op.clone(),
            payload_sha256: owed.payload_sha256.clone(),
            identity: owed.identity.clone(),
            binding: agent::RequestBinding {
                container: container.clone(),
                generation: owed.identity.generation,
                callback: None,
                settles_turn: None,
            },
        };
        {
            let _hot = HotScope::enter();
            let before = work::snapshot();
            let replay = swept(&fx.ctx, &origin, Home::Container(container.clone()), || {
                anyhow::bail!("a stored request must not re-execute")
            })
            .unwrap();
            assert!(replay.replayed);
            assert!(replay.receipt.post_pending.is_empty());
            let after = work::snapshot();
            measured.push(
                [
                    Counter::TaskHeadDecodes,
                    Counter::NestedRowDecodes,
                    Counter::HistoryRowDecodes,
                    Counter::FullTaskHydrations,
                    Counter::DecodedDocBytes,
                    Counter::ViewsOpened,
                ]
                .map(|counter| after.total(counter) - before.total(counter)),
            );
        }
        assert!(
            fx.ctx
                .store
                .head(&id)
                .unwrap()
                .agent_receipt(&container, "owed-event")
                .unwrap()
                .unwrap()
                .post_pending
                .is_empty()
        );
        let events = fx
            .ctx
            .store
            .events_of(Some(&id), &["fixture_effect"])
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].detail["value"], "kept");
    }
    assert_eq!(measured[0], measured[1]);
    assert_eq!(measured[0][3], 0);
}
