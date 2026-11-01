//! Completion's real native-request entry is head/keyed-only, including audited replay.
use super::*;
use corral_core::TaskRef;

pub(super) fn call(ctx: &Ctx, origin: &RequestOrigin) -> anyhow::Result<Receipted> {
    let report = Report {
        origin,
        gate_ref: None,
        contract_sha256: Some("c0".into()),
    };
    let _hot = corral_core::work::HotScope::enter();
    let before = corral_core::work::snapshot();
    let result = crate::review::done_request(
        ctx,
        origin.identity.task.as_deref().unwrap(),
        SUMMARY,
        None,
        &report,
    );
    let delta = corral_core::work::snapshot().since(&before);
    assert_eq!(
        delta.total(corral_core::work::Counter::FullTaskHydrations),
        0,
        "{:?}",
        delta.sites
    );
    result
}

fn committed(origin: &RequestOrigin) -> RequestReceipt {
    serde_json::from_value(json!({
        "version":1,"request_id":origin.request_id,"op":origin.op,
        "payload_sha256":origin.payload_sha256,"identity":origin.identity,
        "state":"committed","at":"2026-01-01T00:00:00Z"
    }))
    .unwrap()
}

#[test]
fn completion_then_same_worker_reopen_appends_without_overwriting_prior_native_records() {
    let fx = fixture(false);
    let generation = fx.bind();
    let old = committed(&fx.call(generation, "before-done", "event"));
    fx.ctx
        .store
        .update_head(&fx.id, |head| {
            head.put_agent_record(&fx.worker(), core_agent::NativeRecord::Receipt(old.clone()))?;
            head.settle_native_inbox(&fx.worker(), "2026-01-01T00:00:00Z".parse()?)?;
            Ok(())
        })
        .unwrap();
    let done = fx.call(generation, "done-1", "done");
    let finished = call(&fx.ctx, &done).unwrap();
    assert!(!finished.replayed);
    assert!(finished.receipt.post_pending.is_empty());
    let head = fx.ctx.store.head(&fx.id).unwrap();
    let host = head.agent_head(&fx.worker()).unwrap().unwrap();
    assert_eq!(host.records.counts, [1, 2, 0, 0, 0, 1, 0, 0, 0, 0]);
    assert_eq!(host.records.pending, 0);
    assert!(
        head.pending_agent_receipts(&fx.worker())
            .unwrap()
            .is_empty()
    );
    assert!(head.unfinished_native_containers().unwrap().is_empty());
    assert!(fx.ctx.store.unfinished_native_heads().unwrap().is_empty());
    // A changes round reopens this worker, not a replacement full-task save that
    // could accidentally repair its projection before the next keyed append.
    fx.ctx
        .store
        .update_head(&fx.id, |head| {
            head.status = Status::Running;
            let run = head.current_run_mut().unwrap();
            run.ended = None;
            run.outcome = None;
            Ok(())
        })
        .unwrap();
    let next_generation = fx.bind();
    assert_eq!(next_generation, generation + 1);
    let next = committed(&fx.call(next_generation, "after-done", "event"));
    fx.ctx
        .store
        .update_head(&fx.id, |head| {
            head.put_agent_record(
                &fx.worker(),
                core_agent::NativeRecord::Receipt(next.clone()),
            )
            .map(drop)
        })
        .unwrap();
    let full = fx.task();
    let native = full.runs[0].agent_host.as_ref().unwrap();
    assert_eq!(native.receipts, vec![old, finished.receipt, next]);
    assert_eq!(native.reports.len(), 1);
    assert_eq!(
        native.generations.iter().map(|g| g.n).collect::<Vec<_>>(),
        vec![generation, next_generation]
    );
    assert_eq!(
        fx.ctx
            .store
            .head(&fx.id)
            .unwrap()
            .agent_head(&fx.worker())
            .unwrap()
            .unwrap(),
        corral_core::AgentHostHead::of(native)
    );
}

#[test]
fn real_done_effect_drain_removes_terminal_current_and_historical_owners_under_receipt_growth() {
    use corral_core::work::{self, Counter, HotScope};
    for historical in [false, true] {
        let mut budgets = Vec::new();
        for scale in [1, 10] {
            let fx = fixture(false);
            fx.ctx
                .store
                .fixture_pin_now("2026-01-01T00:00:00Z".parse().unwrap())
                .unwrap();
            let generation = fx.bind();
            let origin = fx.call(generation, "owed-done", "done");
            let mut owed = committed(&origin);
            owed.result =
                json!({"event":{"kind":"done","detail":{"request_id":origin.request_id}}});
            owed.post_pending.push("effect:event".into());
            fx.ctx
                .store
                .update(&fx.id, |task| {
                    let at = "2026-01-01T00:00:00Z".parse()?;
                    task.created = at;
                    task.status_since = Some(at);
                    task.status = Status::Done;
                    let run = &mut task.runs[0];
                    run.ended = Some(at);
                    run.outcome = Some(RunOutcome::Done);
                    run.inbox.settled = Some(at);
                    let native = run.agent_host.as_mut().unwrap();
                    native.generations[0].created = at;
                    native.receipts = (0..2 * scale)
                        .map(|n| committed(&fx.call(generation, &format!("old-{n:04}"), "done")))
                        .collect();
                    native.receipts.push(owed.clone());
                    if historical {
                        let mut next = super::run("R2", &fx.root().join("app"), "w0:p2");
                        next.ended = Some(at);
                        next.outcome = Some(RunOutcome::Done);
                        next.inbox.settled = Some(at);
                        task.runs.push(next);
                    }
                    Ok(())
                })
                .unwrap();
            assert_eq!(fx.ctx.store.unfinished_native_heads().unwrap().len(), 1);
            let expected = fx.task().runs[0]
                .agent_host
                .as_ref()
                .unwrap()
                .receipts
                .clone();
            {
                let _hot = HotScope::enter();
                let before = work::snapshot();
                let drained = call(&fx.ctx, &origin).unwrap();
                assert!(drained.replayed);
                assert!(drained.receipt.post_pending.is_empty());
                let head = fx.ctx.store.head(&fx.id).unwrap();
                assert!(
                    head.pending_agent_receipts(&fx.worker())
                        .unwrap()
                        .is_empty()
                );
                assert_eq!(
                    head.agent_head(&fx.worker())
                        .unwrap()
                        .unwrap()
                        .records
                        .pending,
                    0
                );
                assert!(head.unfinished_native_containers().unwrap().is_empty());
                assert!(fx.ctx.store.unfinished_native_heads().unwrap().is_empty());
                let delta = work::snapshot().since(&before);
                delta.check_identity().unwrap();
                assert_eq!(delta.total(Counter::FullTaskHydrations), 0);
                budgets.push(delta);
            }
            let actual = fx.task().runs[0]
                .agent_host
                .as_ref()
                .unwrap()
                .receipts
                .clone();
            assert_eq!(&actual[..2 * scale], &expected[..2 * scale]);
            assert_eq!(actual.len(), expected.len());
            assert_eq!(actual.last().unwrap().request_id, origin.request_id);
            assert!(actual.last().unwrap().post_pending.is_empty());
            let replay = call(&fx.ctx, &origin).unwrap();
            assert!(replay.replayed);
            assert!(replay.receipt.post_pending.is_empty());
            assert_eq!(fx.events("done", &origin.request_id), (1, 1));
        }
        assert_eq!(budgets[0], budgets[1]);
    }
}
