use super::*;

#[test]
fn request_verdict_after_backward_wall_step_resolves_changes_round() {
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };
    let ahead = corral_core::clock::now() + jiff::SignedDuration::from_hours(1);
    let wall = Arc::new(AtomicI64::new(ahead.as_second()));
    let source = Arc::clone(&wall);
    corral_core::clock::with_test_wall(
        move || jiff::Timestamp::from_second(source.load(Ordering::SeqCst)).unwrap(),
        || {
            let fx = fixture(false);
            let origin = fx.reviewer();
            let round = corral_core::clock::now();
            fx.ctx
                .store
                .update(&fx.id, |task| {
                    task.runs[0].changes_round = Some(
                        serde_json::from_value(json!({
                            "n": 1, "sha": "old", "at": round
                        }))
                        .unwrap(),
                    );
                    Ok(())
                })
                .unwrap();
            wall.fetch_sub(7200, Ordering::SeqCst);
            let report = Report {
                origin: &origin,
                gate_ref: None,
                contract_sha256: None,
            };
            crate::reviewer::verdict_request(
                &fx.ctx,
                &fx.id,
                Verdict::Approve,
                "addressed",
                None,
                &report,
            )
            .unwrap();
            let task = fx.task();
            let run = task.current_run().unwrap();
            assert!(run.review.as_ref().unwrap().at > round);
            assert_eq!(
                crate::gate_tier::current_phase(&corral_core::RunHead::of(run).unwrap()),
                crate::GatePhase::Landing
            );
        },
    );
}
