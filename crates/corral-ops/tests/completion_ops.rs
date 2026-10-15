use std::path::{Path, PathBuf};
mod support;

use corral_core::{
    CriteriaManifest, Criterion, EvidenceObservation, ObservationStatus, Run, Status, Store,
};
use corral_ops::Ctx;
use corral_ops::{
    add_artifact, completion_check, completion_check_task, completion_spec_digest, evidence_list,
    record_evidence, register_criteria,
};

const SUMMARY: &str = "Verified: fixture\nNot verified: semantic review\n";

#[test]
fn no_gate_evidence_survives_semantic_review_start() {
    // Given evidence captured before a review under an explicit no-gate policy.
    let fx = Fixture::new();
    let evidence = record_evidence(&fx.ctx, &fx.id, &fx.observation()).unwrap();
    // When semantic review starts with its own attempt identity.
    fx.ctx
        .store
        .update(&fx.id, |task| {
            let mut identity = evidence.identity.clone();
            identity.attempt = "review-attempt".into();
            task.current_run_mut().unwrap().review = Some(corral_core::ReviewRecord {
                sha: identity.head.clone(),
                identity: Some(identity),
                evidence_digest: None,
                verdict: None,
                process_lost: false,
                reviewer: "fixture".into(),
                notes: None,
                at: "2026-01-03T00:00:00Z".parse().unwrap(),
                pane: None,
            });
            Ok(())
        })
        .unwrap();
    // Then starting review does not change the evidence's no-gate attempt.
    let failures = fx.failures();
    assert!(failures.is_empty(), "{failures:?}");
}

struct Fixture {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    id: String,
    app: PathBuf,
    artifact: String,
}

fn git(path: &Path, args: &[&str]) {
    let out = support::command("git", path)
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        std::fs::create_dir(&app).unwrap();
        git(&app, &["init", "-q", "-b", "main"]);
        git(&app, &["config", "user.name", "test"]);
        git(&app, &["config", "user.email", "test@example.invalid"]);
        std::fs::write(app.join("code.txt"), "initial\n").unwrap();
        git(&app, &["add", "."]);
        git(&app, &["commit", "-qm", "initial"]);
        let store = Store::init(tmp.path()).unwrap();
        let ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            herdr: corral_herdr::Herdr::with_bin(
                store.root().join("unused-herdr"),
                Some("corral-test".into()),
            ),
            store,
        };
        let task = ctx
            .store
            .create("criterion", Some("app"), Status::Running, "spec")
            .unwrap();
        let base = corral_git::rev_sha(&app, "HEAD").unwrap();
        let run: Run = serde_json::from_value(serde_json::json!({
            "id":"R1", "agent":"generic", "worktree":app,
            "branch":"main", "base":"main", "base_sha":base,
            "started":"2026-01-01T00:00:00Z"
        }))
        .unwrap();
        ctx.store
            .update(&task.id, |t| {
                t.runs.push(run.clone());
                Ok(())
            })
            .unwrap();
        let source = tmp.path().join("proof.log");
        std::fs::write(&source, "actual fixture bytes\n").unwrap();
        let artifact = add_artifact(&ctx, &task.id, &source, None, None)
            .unwrap()
            .path;
        let manifest = CriteriaManifest {
            version: 1,
            spec_digest: completion_spec_digest(&ctx.store.get(&task.id).unwrap()).unwrap(),
            reviewed_by: "owner".into(),
            audited: true,
            require_gate: false,
            criteria: vec![Criterion {
                id: "C1".into(),
                required: true,
                live: true,
                scenario: "fixture result".into(),
                expected: "zero failures".into(),
            }],
            owner_decisions: vec![],
        };
        register_criteria(&ctx, &task.id, &manifest).unwrap();
        Self {
            _tmp: tmp,
            ctx,
            id: task.id,
            app,
            artifact,
        }
    }

    fn observation(&self) -> EvidenceObservation {
        EvidenceObservation {
            version: 1,
            key: "capture-1".into(),
            criterion: "C1".into(),
            run: "R1".into(),
            attempt: "no-gate".into(),
            status: ObservationStatus::Pass,
            action: "fixture observation".into(),
            environment: "isolated".into(),
            expected: "zero failures".into(),
            observed: "zero failures".into(),
            artifacts: vec![self.artifact.clone()],
            captured: "2026-01-02T00:00:00Z".parse().unwrap(),
            producer: "generic worker".into(),
            live: true,
            cleanup: None,
        }
    }

    fn failures(&self) -> Vec<String> {
        completion_check(&self.ctx, &self.id, SUMMARY)
            .unwrap()
            .failures
    }
}

#[test]
fn identical_observation_is_idempotent_in_canonical_store() {
    // Given one recorded assertion.
    let fx = Fixture::new();
    let observation = fx.observation();
    let original = record_evidence(&fx.ctx, &fx.id, &observation).unwrap();
    // When repeating the same request.
    let repeated = record_evidence(&fx.ctx, &fx.id, &observation).unwrap();
    // Then there is one canonical observation and one event.
    assert_eq!(original, repeated);
    assert_eq!(evidence_list(&fx.ctx, &fx.id).unwrap().len(), 1);
    assert_eq!(
        fx.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "evidence_recorded")
            .count(),
        1
    );
}

#[test]
fn conflicting_key_cannot_replace_existing_observation() {
    // Given a recorded key.
    let fx = Fixture::new();
    let mut observation = fx.observation();
    record_evidence(&fx.ctx, &fx.id, &observation).unwrap();
    observation.observed = "different".into();
    // When reusing it with different content.
    let result = record_evidence(&fx.ctx, &fx.id, &observation);
    // Then the original remains intact.
    assert!(result.is_err());
    assert_eq!(
        evidence_list(&fx.ctx, &fx.id).unwrap()[0]
            .observation
            .observed,
        "zero failures"
    );
}

#[test]
fn concurrent_criterion_writers_preserve_both_observations() {
    // Given two criteria and two writers released by the same explicit barrier.
    let fx = Fixture::new();
    let mut manifest = fx.ctx.store.get(&fx.id).unwrap().completion.unwrap();
    let mut second = manifest.criteria[0].clone();
    second.id = "C2".into();
    manifest.criteria.push(second);
    register_criteria(&fx.ctx, &fx.id, &manifest).unwrap();
    let first = fx.observation();
    let mut second = first.clone();
    second.key = "capture-2".into();
    second.criterion = "C2".into();
    let barrier = std::sync::Barrier::new(3);
    // When both record through the actual store transaction.
    std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            record_evidence(&fx.ctx, &fx.id, &first).unwrap()
        });
        let b = scope.spawn(|| {
            barrier.wait();
            record_evidence(&fx.ctx, &fx.id, &second).unwrap()
        });
        barrier.wait();
        a.join().unwrap();
        b.join().unwrap();
    });
    // Then no last-writer overwrite loses a criterion.
    let records = evidence_list(&fx.ctx, &fx.id).unwrap();
    let mut ids: Vec<_> = records
        .iter()
        .map(|r| r.observation.criterion.as_str())
        .collect();
    ids.sort();
    assert_eq!(ids, ["C1", "C2"]);
}

#[test]
fn malformed_and_cross_run_references_never_append() {
    // Given valid inputs with individual boundary violations.
    let fx = Fixture::new();
    let base = fx.observation();
    let mut inputs = Vec::new();
    let mut o = base.clone();
    o.run = "R2".into();
    inputs.push(o);
    let mut o = base.clone();
    o.criterion = "unknown".into();
    inputs.push(o);
    let mut o = base.clone();
    o.key = "../escape".into();
    inputs.push(o);
    let mut o = base.clone();
    o.attempt = "other".into();
    inputs.push(o);
    let mut o = base.clone();
    o.artifacts = vec!["../outside".into()];
    inputs.push(o);
    let mut o = base.clone();
    o.artifacts = vec![fx.artifact.replace("/R1/", "/R2/")];
    inputs.push(o);
    let mut o = base.clone();
    o.artifacts.clear();
    inputs.push(o);
    let mut o = base;
    o.action.clear();
    inputs.push(o);
    // When validating each request.
    let results: Vec<_> = inputs
        .iter()
        .map(|o| record_evidence(&fx.ctx, &fx.id, o))
        .collect();
    // Then none writes a record.
    assert!(results.iter().all(Result::is_err));
    assert!(evidence_list(&fx.ctx, &fx.id).unwrap().is_empty());
}

#[test]
fn tampered_artifact_is_rejected_at_record_boundary() {
    // Given a snapshot whose bytes changed.
    let fx = Fixture::new();
    std::fs::write(fx.ctx.store.dir().join(&fx.artifact), "tampered").unwrap();
    // When registering its observation.
    let result = record_evidence(&fx.ctx, &fx.id, &fx.observation());
    // Then no observation is accepted.
    assert!(result.is_err());
    assert!(evidence_list(&fx.ctx, &fx.id).unwrap().is_empty());
}

#[test]
fn empty_artifact_is_rejected_at_record_boundary() {
    // Given an empty registered snapshot.
    let fx = Fixture::new();
    let source = fx._tmp.path().join("empty");
    std::fs::write(&source, "").unwrap();
    let a = add_artifact(&fx.ctx, &fx.id, &source, None, None).unwrap();
    let mut observation = fx.observation();
    observation.artifacts = vec![a.path];
    // When registering evidence.
    let result = record_evidence(&fx.ctx, &fx.id, &observation);
    // Then an empty snapshot cannot count as proof.
    assert!(result.is_err());
}

#[test]
fn required_live_evidence_rejects_offline_assertions() {
    // Given a required live criterion.
    let fx = Fixture::new();
    let mut o = fx.observation();
    o.live = false;
    record_evidence(&fx.ctx, &fx.id, &o).unwrap();
    // When auditing.
    let failures = fx.failures();
    // Then offline presence cannot satisfy it.
    assert!(failures.contains(&"evidence_not_live:C1".into()));
}

#[test]
fn latest_not_run_does_not_reuse_previous_pass() {
    // Given a pass followed by a not-run observation.
    let fx = Fixture::new();
    let mut o = fx.observation();
    record_evidence(&fx.ctx, &fx.id, &o).unwrap();
    o.key = "capture-2".into();
    o.status = ObservationStatus::NotRun;
    record_evidence(&fx.ctx, &fx.id, &o).unwrap();
    // When auditing.
    let failures = fx.failures();
    // Then the earlier pass is not promoted.
    assert!(failures.contains(&"evidence_not_run:C1".into()));
}

#[test]
fn committed_change_makes_old_evidence_stale() {
    // Given an observation at the old HEAD.
    let fx = Fixture::new();
    record_evidence(&fx.ctx, &fx.id, &fx.observation()).unwrap();
    std::fs::write(fx.app.join("code.txt"), "changed\n").unwrap();
    git(&fx.app, &["commit", "-qam", "change"]);
    // When auditing the new HEAD.
    let failures = fx.failures();
    // Then old evidence cannot establish current presence.
    assert!(failures.contains(&"evidence_stale:C1".into()));
}

#[test]
fn spec_edits_require_explicit_manifest_reregistration() {
    // Given a manifest bound to the previous specification.
    let fx = Fixture::new();
    fx.ctx
        .store
        .update(&fx.id, |t| {
            t.body.push_str("\nnew requirement");
            Ok(())
        })
        .unwrap();
    // When checking completion.
    let failures = fx.failures();
    // Then the mapping is stale, not automatically reconciled.
    assert!(failures.contains(&"manifest_spec_stale".into()));
}

#[test]
fn successful_preflight_is_read_only_and_lock_safe() {
    // Given all required evidence under explicit no-gate policy.
    let fx = Fixture::new();
    record_evidence(&fx.ctx, &fx.id, &fx.observation()).unwrap();
    let task = fx.ctx.store.get(&fx.id).unwrap();
    let before = std::fs::read(&task.path).unwrap();
    let events = fx.ctx.store.events().unwrap().len();
    // When auditing without a transition.
    let audit = completion_check_task(&fx.ctx, &task, SUMMARY).unwrap();
    // Then identity and evidence are reported without writes.
    assert!(audit.passed, "{:?}", audit.failures);
    assert!(audit.audited);
    assert_eq!(audit.evidence_keys, ["capture-1"]);
    assert_eq!(std::fs::read(&task.path).unwrap(), before);
    assert_eq!(fx.ctx.store.events().unwrap().len(), events);
}

#[test]
fn dirty_candidate_and_missing_gate_are_named_failures() {
    // Given a dirty candidate and required gate.
    let fx = Fixture::new();
    let task = fx.ctx.store.get(&fx.id).unwrap();
    let mut manifest = task.completion.unwrap();
    manifest.require_gate = true;
    register_criteria(&fx.ctx, &fx.id, &manifest).unwrap();
    std::fs::write(fx.app.join("untracked"), "dirty").unwrap();
    // When auditing.
    let failures = fx.failures();
    // Then both independent blockers remain visible.
    assert!(failures.contains(&"worktree_dirty".into()));
    assert!(failures.contains(&"gate_missing".into()));
}

#[test]
fn legacy_completion_does_not_gain_audit_requirements() {
    // Given a legacy task.
    let fx = Fixture::new();
    fx.ctx
        .store
        .update(&fx.id, |t| {
            t.completion = None;
            t.current_run_mut().unwrap().completion = Default::default();
            Ok(())
        })
        .unwrap();
    // When checking an ordinary summary.
    let audit = completion_check(&fx.ctx, &fx.id, "legacy").unwrap();
    // Then the legacy path remains explicit.
    assert!(audit.passed);
    assert!(!audit.audited);
}

#[test]
fn unresolved_owner_decisions_block_audited_completion() {
    // Given an unresolved owner decision.
    let fx = Fixture::new();
    let task = fx.ctx.store.get(&fx.id).unwrap();
    let mut manifest = task.completion.unwrap();
    manifest.owner_decisions.push("D1".into());
    register_criteria(&fx.ctx, &fx.id, &manifest).unwrap();
    record_evidence(&fx.ctx, &fx.id, &fx.observation()).unwrap();
    // When auditing.
    let failures = fx.failures();
    // Then it remains an explicit blocker.
    assert!(failures.contains(&"owner_decision:D1".into()));
}

#[test]
fn native_run_owner_decisions_block_audited_completion() {
    // Given an audited run with evidence and an unanswered T-057 decision.
    let fx = Fixture::new();
    record_evidence(&fx.ctx, &fx.id, &fx.observation()).unwrap();
    let decision = corral_core::DecisionRecord {
        request: corral_core::DecisionRequest {
            version: 1,
            id: "native-decision".into(),
            run: "R1".into(),
            question: "Which option?".into(),
            requested_action: "Choose an option".into(),
            owner: "owner".into(),
            source: corral_core::ObligationSource::Worker,
            deadline: None,
        },
        registered_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        observation: corral_core::WaitObservation {
            at: "2026-01-01T00:00:00Z".parse().unwrap(),
            state: corral_core::WaitState::Pending,
            detail: "awaiting owner".into(),
        },
        receipt: None,
    };
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut()
                .unwrap()
                .obligations
                .decisions
                .push(decision);
            Ok(())
        })
        .unwrap();
    // When audited completion reads the existing obligation ledger.
    let failures = fx.failures();
    // Then it does not require a duplicate manifest declaration to block intake.
    assert!(failures.contains(&"owner_decision:native-decision".into()));
}

#[test]
fn artifact_deletion_after_registration_blocks_completion() {
    // Given a registered observation whose snapshot disappears.
    let fx = Fixture::new();
    record_evidence(&fx.ctx, &fx.id, &fx.observation()).unwrap();
    std::fs::remove_file(fx.ctx.store.dir().join(&fx.artifact)).unwrap();
    // When auditing.
    let failures = fx.failures();
    // Then metadata alone cannot satisfy evidence presence.
    assert!(failures.contains(&"evidence_artifact_invalid:C1".into()));
}

#[test]
fn captures_outside_the_run_are_rejected_without_mutation() {
    for captured in ["2025-12-31T23:59:59Z", "9999-01-01T00:00:00Z"] {
        // Given an otherwise valid observation outside its run's time window.
        let fx = Fixture::new();
        let mut observation = fx.observation();
        observation.captured = captured.parse().unwrap();
        let before = fx.ctx.store.get(&fx.id).unwrap();
        let events = fx.ctx.store.events().unwrap();

        // When recording a pre-run or future capture.
        let error = record_evidence(&fx.ctx, &fx.id, &observation).unwrap_err();

        // Then ownership fails closed without appending evidence or events.
        assert_eq!(
            error.to_string().split(':').next().unwrap(),
            "capture_outside_attempt"
        );
        assert_eq!(fx.ctx.store.get(&fx.id).unwrap(), before);
        assert_eq!(fx.ctx.store.events().unwrap(), events);
    }
}

#[test]
fn old_capture_cannot_satisfy_a_new_attempt() {
    // Given evidence captured before the current gate finished.
    let fx = Fixture::new();
    let record = record_evidence(&fx.ctx, &fx.id, &fx.observation()).unwrap();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut().unwrap().gate = Some(corral_core::GateRecord {
                identity: Some(record.identity.clone()),
                commands: vec![],
                log_sha256: None,
                result: corral_core::GateResult::Pass,
                process_lost: false,
                failed_cmd: None,
                at: "2026-01-03T00:00:00Z".parse().unwrap(),
                sha: record.identity.head.clone(),
                pane: None,
            });
            Ok(())
        })
        .unwrap();
    // When checking evidence for that gate.
    let task = fx.ctx.store.get(&fx.id).unwrap();
    let failures =
        corral_ops::evidence_failures(&fx.ctx, &task, task.current_run().unwrap()).unwrap();
    // Then a matching tree cannot relabel an older capture.
    assert!(failures.contains(&"evidence_capture_stale:C1".into()));
}

#[test]
fn old_snapshot_cannot_be_reattached_to_new_attempt() {
    // Given an existing snapshot and a gate with a strictly later completion time.
    let fx = Fixture::new();
    let identity = completion_check(&fx.ctx, &fx.id, SUMMARY)
        .unwrap()
        .identity
        .unwrap();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut().unwrap().gate = Some(corral_core::GateRecord {
                identity: Some(identity.clone()),
                commands: vec![],
                log_sha256: None,
                result: corral_core::GateResult::Pass,
                process_lost: false,
                failed_cmd: None,
                at: "9999-01-01T00:00:00Z".parse().unwrap(),
                sha: identity.head.clone(),
                pane: None,
            });
            Ok(())
        })
        .unwrap();
    // When recording a new assertion over the old snapshot.
    let error = record_evidence(&fx.ctx, &fx.id, &fx.observation()).unwrap_err();
    // Then the snapshot boundary rejects it before promoting any observation.
    assert!(error.to_string().starts_with("artifact_before_attempt:"));
    assert!(evidence_list(&fx.ctx, &fx.id).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn snapshot_symlink_cannot_escape_the_run_directory() {
    // Given a snapshot replaced by an outside symlink with identical bytes.
    let fx = Fixture::new();
    let snapshot = fx.ctx.store.dir().join(&fx.artifact);
    let outside = fx._tmp.path().join("outside");
    std::fs::copy(&snapshot, &outside).unwrap();
    std::fs::remove_file(&snapshot).unwrap();
    std::os::unix::fs::symlink(&outside, &snapshot).unwrap();
    // When recording its reference.
    let result = record_evidence(&fx.ctx, &fx.id, &fx.observation());
    // Then matching content does not authorize a path escape.
    assert!(result.is_err());
}
