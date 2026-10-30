//! Actual artifact producer interrupted after its commit, then recovered through the hot sweep.
use super::*;
use crate::agent::{native_test_support::data, test_support as ts};
use corral_core::{
    ArtifactKind, PinRef, RunOutcome,
    work::{self, Counter, HotScope},
};

#[test]
fn artifact_effect_recovery_indexes_only_its_registration_with_fixed_work() {
    let mut costs = Vec::new();
    let mut replay_costs = Vec::new();
    for scale in [1, 10] {
        for warm in [false, true] {
            let at = data::at(100_000);
            corral_core::clock::pin_thread(Some(at));
            let fx = ts::fx();
            fx.ctx.store.fixture_pin_now(at).unwrap();
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
            ts::committed(&ts::hello(&fx, &host));
            let container = ContainerRef::Worker {
                task: id.clone(),
                run: "R1".into(),
            };
            let mut task = fx.ctx.store.get(&id).unwrap();
            let current = task.current_run().unwrap().clone();
            for i in 0..2 * scale {
                let mut old = current.clone();
                old.id = format!("old-{i:04}");
                old.outcome = Some(RunOutcome::Done);
                old.ended = Some(at);
                old.inbox.settled = Some(at);
                old.agent_host = None;
                task.runs.insert(0, old);
            }
            fx.ctx.store.fixture_put_task(&task).unwrap();
            for i in 0..2 * scale {
                let old = fx.ctx.store.root().join(format!("old-{i:04}.txt"));
                std::fs::write(&old, format!("retained proof {i:04}\n")).unwrap();
                crate::artifacts::add_artifact(
                    &fx.ctx,
                    &id,
                    &old,
                    Some(ArtifactKind::Log),
                    Some("Retained proof"),
                )
                .unwrap();
                let mut terminal = fx
                    .ctx
                    .store
                    .create(
                        &format!("Retired {i:04}"),
                        Some("app"),
                        corral_core::Status::Done,
                        "old",
                    )
                    .unwrap();
                data::grow(&mut terminal, 2 * scale, 2 * scale, 2 * scale, 2 * scale);
                fx.ctx.store.fixture_put_task(&terminal).unwrap();
            }
            let cache = fx
                .ctx
                .store
                .cache_root()
                .unwrap()
                .join("content-search.json");
            if warm {
                crate::content_index::snapshot(&fx.ctx, &fx.ctx.store.list().unwrap(), false)
                    .unwrap();
            } else if cache.exists() {
                std::fs::remove_file(&cache).unwrap();
            }
            let source = fx.ctx.store.root().join("current-proof.txt");
            std::fs::write(&source, "current proof kept verbatim\n").unwrap();
            let origin = RequestOrigin {
                request_id: "native-artifact".into(),
                op: "artifact".into(),
                payload_sha256: "44".repeat(32),
                identity: host.identity.clone(),
                binding: agent::RequestBinding {
                    container: container.clone(),
                    generation: host.identity.generation,
                    callback: None,
                    settles_turn: None,
                },
            };
            // No receipt/Task facade is invented: this is the real attach+receipt transaction.
            super::super::receipts::FAILPOINT.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(|point| {
                    if point == "swept_after_commit" {
                        panic!("fixture interruption after actual artifact commit");
                    }
                }))
            });
            let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::artifacts::add_artifact_request(
                    &fx.ctx,
                    &id,
                    &source,
                    Some(ArtifactKind::Log),
                    Some("Current proof"),
                    &origin,
                )
            }));
            super::super::receipts::FAILPOINT.with(|hook| *hook.borrow_mut() = None);
            assert!(
                interrupted.is_err(),
                "the producer must reach its committed effect boundary"
            );
            let receipt = fx
                .ctx
                .store
                .head(&id)
                .unwrap()
                .agent_receipt(&container, &origin.request_id)
                .unwrap()
                .unwrap();
            assert!(
                receipt
                    .post_pending
                    .iter()
                    .any(|step| step == "effect:content_index")
            );
            let path = receipt.result["path"].as_str().unwrap();
            let target = fx
                .ctx
                .store
                .head(&id)
                .unwrap()
                .latest_artifact_by_path(path)
                .unwrap()
                .unwrap();
            let reference = PinRef::of_artifact(&id, &target).to_string();
            for first in [true, false] {
                let before = work::snapshot();
                let hot = HotScope::enter();
                reconcile_task_hot(&fx.ctx, &id).unwrap();
                let delta = work::snapshot().since(&before);
                drop(hot);
                delta.check_identity().unwrap();
                assert_eq!(delta.unattributed_hydrations(), 0, "{delta:?}");
                assert!(
                    delta.followup_calls.is_empty(),
                    "ordinary artifact indexing needs no exemption: {delta:?}"
                );
                if first {
                    costs.push(Counter::ALL.map(|counter| delta.get(counter)));
                } else {
                    replay_costs.push(Counter::ALL.map(|counter| delta.get(counter)));
                }
            }
            let receipt = fx
                .ctx
                .store
                .head(&id)
                .unwrap()
                .agent_receipt(&container, &origin.request_id)
                .unwrap()
                .unwrap();
            assert!(receipt.post_pending.is_empty());
            let cached: Value = serde_json::from_slice(&std::fs::read(&cache).unwrap()).unwrap();
            let document = &cached["documents"][&reference];
            assert_eq!(document["text"], "current proof kept verbatim\n");
            assert_eq!(document["title"], "Current proof");
            // The complete public snapshot remains the output oracle, outside the hot callback.
            let full =
                crate::content_index::snapshot(&fx.ctx, &fx.ctx.store.list().unwrap(), false)
                    .unwrap();
            let expected = full
                .records
                .iter()
                .find(|record| record.document.reference == reference)
                .unwrap();
            assert_eq!(document, &serde_json::to_value(&expected.document).unwrap());
            let artifact_events = fx.ctx.store.events_of(Some(&id), &["artifact"]).unwrap();
            assert_eq!(
                artifact_events
                    .iter()
                    .filter(|event| event.detail["path"] == path)
                    .count(),
                1
            );
            eprintln!(
                "actual artifact producer/recovery scale={scale} warm={warm}: {:?}",
                costs.last().unwrap()
            );
            corral_core::clock::pin_thread(None);
        }
    }
    for cost in &costs[1..] {
        assert_eq!(
            cost, &costs[0],
            "every first-recovery counter incl. doc bytes"
        );
    }
    for cost in &replay_costs[1..] {
        assert_eq!(
            cost, &replay_costs[0],
            "every once-only replay counter incl. doc bytes"
        );
    }
}
