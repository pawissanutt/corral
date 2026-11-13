//! A contended inventory still collects owned completion receipts without hydrating history.
use super::*;
use corral_core::work::{self, Counter, HotScope};

#[test]
fn contended_inventory_collects_its_gate_receipt_with_no_unattributed_hydration_or_history_growth()
{
    let now: Timestamp = "2026-10-18T12:00:00Z".parse().unwrap();
    corral_core::clock::pin_thread(Some(now));
    crate::resources::pin_psi(Some((Some(0.25), Some(1.5))));
    let mut results = Vec::new();
    for scale in [1, 10] {
        let mut fx = Fixture::new(2);
        fx.ctx.store.fixture_pin_now(now).unwrap();
        let id = fx.task();
        for i in 0..scale * 2 {
            let mut task = fx
                .ctx
                .store
                .create(&format!("retired-{i:04}"), Some("app"), Status::Done, "")
                .unwrap();
            crate::agent::native_test_support::data::grow(
                &mut task,
                scale * 2,
                scale * 2,
                scale * 2,
                scale * 2,
            );
            fn close(run: &mut corral_core::Run, now: Timestamp, socket: &std::path::Path) {
                run.ended = Some(now);
                run.outcome = Some(corral_core::RunOutcome::Done);
                run.inbox.settled = Some(now);
                run.obligations.slot_wait = None;
                for wait in &mut run.obligations.waits {
                    wait.observation.state = corral_core::WaitState::Cancelled;
                }
                for decision in &mut run.obligations.decisions {
                    decision.observation.state = corral_core::WaitState::Cancelled;
                }
                if let Some(pane) = &mut run.herdr {
                    pane.socket = Some(socket.into());
                }
                if let Some(gate) = &mut run.gate {
                    gate.socket = Some(socket.into());
                    gate.result = corral_core::GateResult::Pass;
                }
                if let Some(review) = &mut run.review {
                    review.verdict = Some(Verdict::Approve);
                }
                for child in &mut run.review_runs {
                    close(child, now, socket);
                }
            }
            let socket = fx.ctx.herdr.socket_path().unwrap();
            for run in &mut task.runs {
                close(run, now, &socket);
            }
            for run in &mut task.plan_runs {
                run.ended = Some(now);
                run.state = corral_core::PlanRunState::Approved;
                run.inbox.settled = Some(now);
                if let Some(pane) = &mut run.herdr {
                    pane.socket = Some(socket.clone());
                }
            }
            for plan in &mut task.plans {
                for review in &mut plan.reviews {
                    review.verdict = Some(Verdict::Approve);
                    review.inbox.settled = Some(now);
                }
            }
            for attempt in &mut task.aggregate_attempts {
                for review in &mut attempt.reviews {
                    review.verdict = Some(Verdict::Approve);
                }
            }
            fx.ctx.store.fixture_put_task(&task).unwrap();
        }
        crate::gate::gate_start(&fx.ctx, &id).unwrap();
        let script = crate::gate::gate_paths(&fx.ctx, &id, "R1").script;
        let ran = support::command("bash", fx.tmp.path())
            .arg(script)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(ran.status.success(), "{ran:?}");
        let probe = std::sync::Arc::new(crate::resources::PinnedReadingsProbe::admitting());
        fx.ctx.resource_probe = probe.clone();
        fx.ctx.store.config.validate(fx.ctx.store.root()).unwrap();
        // Real durable initialization, as sync performs before entering its hot passes.
        // This cannot consume the gate receipt or manufacture a settled task set.
        crate::settling::recover(&fx.ctx).unwrap();
        assert!(crate::settling::terminal_heads(&fx.ctx).unwrap().is_empty());
        // Own the exact inventory-issuance lock: inventory is UNKNOWN, not an empty absence sample.
        let _contended = fx.ctx.store.try_lock("pane-liveness").unwrap().unwrap();
        assert!(crate::pane_liveness::inventory(&fx.ctx).is_none());
        let before = work::snapshot();
        let hot = HotScope::enter();
        crate::sync_at(&fx.ctx, now).unwrap();
        // Observe the durable effect before inspecting counters; UNKNOWN never blocks a receipt.
        let task = fx.ctx.store.head(&id).unwrap();
        assert_eq!(
            task.current_run
                .as_ref()
                .unwrap()
                .gate
                .as_ref()
                .unwrap()
                .result,
            corral_core::GateResult::Pass
        );
        assert_eq!(
            fx.ctx.store.events_of(Some(&id), &["gate"]).unwrap().len(),
            1
        );
        // A healthy pass persists no latch: absent reads as unlatched; present must say so.
        let stored: Option<crate::resources::ResourceState> =
            corral_core::records::RecordArea::new("resources")
                .load(&fx.ctx.store, "state")
                .unwrap();
        assert!(stored.is_none_or(|state| !state.held && state.reason.is_none()));
        let delta = work::snapshot().since(&before);
        drop(hot);
        assert!(probe.samples() > 0);
        assert_eq!(delta.followup_calls("FU-3", "sample"), probe.samples());
        assert_eq!(delta.unattributed_hydrations(), 0, "{:?}", delta.sites);
        delta.check_identity().unwrap();
        // Re-derived outside the measured scope, after the sample count is checked.
        let state = crate::resources::read_status(&fx.ctx).unwrap();
        assert!(!state.held && state.reason.is_none());
        results.push(Counter::ALL.map(|c| delta.get(c)));
    }
    assert_eq!(results[0], results[1]);
}
