use corral_core::{ObservationStatus, StepState};
use corral_ops::{
    completion_check, progress_readback, register_criteria, resume_packet, save_checkpoint,
    update_progress,
};

use super::fixture::Fixture;

#[test]
fn progress_and_checkpoint_reject_unregistered_criterion_ids() {
    for checkpoint in [false, true] {
        // Given a registered canonical criterion and no writes.
        let f = Fixture::new();
        let before = f.snapshot();
        // When a progress or checkpoint link names an unregistered criterion.
        let error = if checkpoint {
            let mut input = f.checkpoint();
            input.criterion_ids = vec!["unknown".into()];
            save_checkpoint(&f.ctx, &f.id, &input).unwrap_err()
        } else {
            let mut input = f.progress();
            input.phases[0].steps[0].criteria = vec!["unknown".into()];
            update_progress(&f.ctx, &f.id, &input).unwrap_err()
        };
        // Then the canonical namespace is enforced without mutation.
        assert_eq!(error.to_string(), "unknown_criterion: unknown");
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn checkpoint_evidence_requires_a_receipt_for_its_linked_criterion() {
    for artifact_only in [false, true] {
        // Given a canonical receipt linked by key, not snapshot path.
        let f = Fixture::new();
        f.seed_checkpoint();
        let mut input = f.checkpoint();
        let task = f.task();
        let record = &task.current_run().unwrap().completion.evidence[0];
        input.evidence_refs = vec![if artifact_only {
            record.observation.artifacts[0].clone()
        } else {
            input.criterion_ids.clear();
            record.observation.key.clone()
        }];
        let before = f.snapshot();
        // When the checkpoint names only an artifact or omits the receipt's criterion.
        let error = save_checkpoint(&f.ctx, &f.id, &input).unwrap_err();
        // Then no duplicate artifact-based evidence namespace is accepted.
        let code = if artifact_only {
            "unknown_evidence_reference"
        } else {
            "evidence_criterion_mismatch"
        };
        assert_eq!(error.to_string().split(':').next().unwrap(), code);
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn readback_and_resume_join_steps_to_canonical_receipts() {
    // Given criterion-linked progress with a receipt-backed checkpoint.
    let f = Fixture::new();
    f.seed_checkpoint();
    let task = f.task();
    let run = task.current_run().unwrap();
    // When readback and a fresh run's resume brief are built.
    let read = progress_readback(&f.ctx, &f.id).unwrap();
    let mut next = run.clone();
    next.id = "R2".into();
    next.completion.evidence.clear();
    let packet = resume_packet(&f.ctx, &task, &next).unwrap();
    // Then both expose the canonical objects and source-run receipt provenance.
    assert_eq!(
        read.criteria[0].criterion,
        task.completion.as_ref().unwrap().criteria[0]
    );
    assert_eq!(read.criteria[0].steps.len(), 3);
    assert_eq!(read.criteria[0].evidence, run.completion.evidence);
    assert_eq!(packet.criteria, read.criteria);
    assert_eq!(packet.progress, run.progress);
    assert!(packet.reusable_evidence.is_empty());
    assert_eq!(packet.criteria[0].evidence[0].identity.run, "R1");
}

#[test]
fn completion_and_readback_flag_complete_progress_without_valid_evidence() {
    for audited in [false, true] {
        for receipt in ["missing", "pass", "fail", "stale", "not_live"] {
            // Given execution-complete progress, including an optional criterion.
            let f = Fixture::new();
            let mut manifest = f.task().completion.unwrap();
            manifest.audited = audited;
            manifest.criteria[0].required = false;
            register_criteria(&f.ctx, &f.id, &manifest).unwrap();
            if receipt != "missing" {
                f.seed_checkpoint();
            }
            let mut input = f.progress();
            for step in input.phases.iter_mut().flat_map(|p| &mut p.steps) {
                step.status = StepState::Done;
            }
            update_progress(&f.ctx, &f.id, &input).unwrap();
            f.ctx
                .store
                .update(&f.id, |task| {
                    let run = task.current_run_mut().unwrap();
                    match receipt {
                        "fail" => {
                            run.completion.evidence[0].observation.status = ObservationStatus::Fail
                        }
                        "stale" => run.completion.evidence[0].identity.head = "obsolete".into(),
                        "not_live" => run.completion.evidence[0].observation.live = false,
                        "missing" | "pass" => {}
                        _ => unreachable!(),
                    }
                    Ok(())
                })
                .unwrap();
            // When done preflight and progress readback assess the same ledger.
            let audit =
                completion_check(&f.ctx, &f.id, "Verified: fixture\nNot verified: none").unwrap();
            let read = progress_readback(&f.ctx, &f.id).unwrap();
            // Then valid evidence alone clears the mismatch in both surfaces.
            let mismatch = "progress_without_evidence:C-1".to_string();
            assert_eq!(
                audit.failures.contains(&mismatch),
                receipt != "pass",
                "{audited} {receipt}"
            );
            assert_eq!(read.evidence_failures, audit.failures);
            assert_eq!(audit.passed, receipt == "pass");
        }
    }
}

#[test]
fn changed_manifest_makes_checkpoint_progress_historical() {
    // Given a checkpoint for the original canonical criterion definition.
    let f = Fixture::new();
    f.seed_checkpoint();
    let mut manifest = f.task().completion.unwrap();
    manifest.criteria[0].expected = "a different result".into();
    register_criteria(&f.ctx, &f.id, &manifest).unwrap();
    // When a worker receives the reconciled task.
    let packet = f.packet();
    // Then old progress cannot claim compatibility with the new criteria contract.
    assert_eq!(packet["status"], "stale");
    assert!(packet["progress"].is_null());
    assert!(
        packet["reasons"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("spec_changed"))
    );
}
