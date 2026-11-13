//! Promotion invariants with explicit evidence-backed diagnostic reruns.
use super::*;
use corral_ops::gate_triage::{self, FailureClass, GateDiagnosis};

fn investigate(fx: &Fx, id: &str, reason: &str) {
    let gate = fx.run(id).gate.unwrap();
    let attempt = gate.identity.unwrap().attempt;
    let file = fx.out.join(format!("investigation-{attempt}.md"));
    std::fs::write(&file, reason).unwrap();
    let artifact = corral_ops::add_artifact(
        &fx.ctx,
        id,
        &file,
        Some(ArtifactKind::Report),
        Some("Owner fixture investigation"),
    )
    .unwrap();
    gate_triage::diagnose(
        &fx.ctx,
        id,
        GateDiagnosis {
            attempt,
            class: FailureClass::Environment,
            explanation: reason.into(),
            evidence: vec![artifact.path],
            baseline: None,
            retry: true,
        },
        true,
        &corral_ops::command::Invoker::process(),
    )
    .unwrap();
}

#[test]
fn full_command_failure_clears_the_origin_but_invalid_receipts_keep_it() {
    let fx = Fx::new(true, |_| {});
    let (id, origin) = approved_on_quick(&fx);
    // Unlike a gate-only fixture's still-running fake worker, diagnosis concerns a
    // submitted candidate with no active author/check.
    fx.ctx
        .store
        .update(&id, |task| {
            task.status = Status::Review;
            let run = task.current_run_mut().unwrap();
            run.outcome = Some(corral_core::RunOutcome::Done);
            run.ended = Some(fx.ctx.store.now());
            Ok(())
        })
        .unwrap();
    // An invalid receipt is not a test result: the origin stays.
    let b = fx.start(&id);
    assert_promotable(&fx, &id, &b, &origin);
    let log = corral_ops::gate_paths(&fx.ctx, &id, &fx.run(&id).id).log;
    let mut bytes = std::fs::read(&log).unwrap();
    bytes.extend_from_slice(b"tampered\n");
    std::fs::write(&log, bytes).unwrap();
    let invalid = corral_ops::gate_collect(&fx.ctx, &id).unwrap().unwrap();
    assert_eq!(
        invalid.failed_cmd.as_deref(),
        Some("invalid_command_receipts")
    );
    assert_eq!(fx.run(&id).promotion_origin.as_deref(), Some(&origin));
    investigate(
        &fx,
        &id,
        "The fixture deliberately corrupted the transport log, not product code; authorize one bounded diagnostic check.",
    );
    // A full landing gate whose command fails clears it in its collection update.
    std::fs::write(fx.out.join("full-fails"), "").unwrap();
    let failed = fx.gate(&id);
    assert_eq!(
        failed.failed_cmd.as_deref(),
        Some(full_command(&fx.out).as_str())
    );
    assert_eq!(fx.run(&id).promotion_origin, None);
    investigate(
        &fx,
        &id,
        "The outside-checkout full-fails marker deliberately caused this command failure; removing it is an environment repair, not a code fix. Authorize one more owner investigation.",
    );
    // A later full PASS is never promoted from the quick approval.
    std::fs::remove_file(fx.out.join("full-fails")).unwrap();
    let pass = fx.gate(&id);
    assert_eq!(pass.result, GateResult::Pass);
    assert_eq!(pass.tier.as_ref().unwrap().promoted_from, None);
    assert_eq!(fx.run(&id).promotion_origin, None);
}
