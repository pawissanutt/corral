use super::*;
use corral_core::{
    CriteriaManifest, Criterion, EvidenceObservation, ObservationStatus, Run, Status,
};
use std::path::Path;

fn git(repo: &Path, args: &[&str]) {
    let out = crate::test_support::command("git", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
}
struct Fixture {
    _dir: tempfile::TempDir,
    ctx: Ctx,
    id: String,
    root: std::path::PathBuf,
    artifact: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "-qb", "main"]);
        git(&root, &["config", "user.name", "Fixture"]);
        git(&root, &["config", "user.email", "fixture@example.invalid"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("code.txt"), "base\n").unwrap();
        corral_git::commit_all(&root, "base").unwrap();
        let base = corral_git::rev_sha(&root, "HEAD").unwrap();
        git(&root, &["checkout", "-qb", "candidate"]);
        std::fs::write(root.join("code.txt"), "candidate\n").unwrap();
        corral_git::commit_all(&root, "candidate").unwrap();
        let ctx = crate::resources::FixedResourceProbe::ctx(
            corral_core::Store::init(dir.path()).unwrap(),
        );
        let task = ctx
            .store
            .create("inspection", Some("app"), Status::Running, "spec")
            .unwrap();
        let run: Run = serde_json::from_value(serde_json::json!({
            "id":"R1", "agent":"generic", "worktree":root, "branch":"candidate", "base":"main",
            "base_sha":base, "started":"2026-01-01T00:00:00Z"
        }))
        .unwrap();
        ctx.store
            .update(&task.id, |t| {
                t.runs.push(run.clone());
                Ok(())
            })
            .unwrap();
        let source = dir.path().join("proof.log");
        std::fs::write(&source, "proof\n").unwrap();
        let artifact = crate::add_artifact(&ctx, &task.id, &source, None, None)
            .unwrap()
            .path;
        Self {
            _dir: dir,
            ctx,
            id: task.id,
            root,
            artifact,
        }
    }
    fn task(&self) -> Task {
        self.ctx.store.get(&self.id).unwrap()
    }
    fn register(&self, audited: bool) {
        let manifest = CriteriaManifest {
            version: 1,
            spec_digest: completion::completion_spec_digest(&self.task()).unwrap(),
            reviewed_by: "owner".into(),
            audited,
            require_gate: false,
            criteria: vec![Criterion {
                id: "C1".into(),
                required: audited,
                live: true,
                scenario: "Inspect real result".into(),
                expected: "proof".into(),
            }],
            owner_decisions: vec![],
        };
        crate::register_criteria(&self.ctx, &self.id, &manifest).unwrap();
    }
    fn record(&self, status: ObservationStatus) {
        let observation = EvidenceObservation {
            version: 1,
            key: "proof-1".into(),
            criterion: "C1".into(),
            run: "R1".into(),
            attempt: "no-gate".into(),
            status,
            action: "inspect".into(),
            environment: "isolated".into(),
            expected: "proof".into(),
            observed: "proof".into(),
            artifacts: vec![self.artifact.clone()],
            captured: "2026-01-02T00:00:00Z".parse().unwrap(),
            producer: "worker".into(),
            live: true,
            cleanup: None,
        };
        crate::record_evidence(&self.ctx, &self.id, &observation).unwrap();
    }
    fn inspect(&self) -> Inspection {
        inspect(&self.ctx, &self.id, "R1").unwrap()
    }
}

#[test]
fn candidate_projection_is_read_only_and_distinguishes_legacy_missing_and_assertions() {
    let f = Fixture::new();
    let before = f.task();
    let legacy = f.inspect();
    assert!(legacy.manifest_digest.is_none());
    assert!(legacy.criteria.is_empty());
    assert!(legacy.gate.is_none());
    assert!(legacy.review.is_none());
    assert_ne!(legacy.base, legacy.head);
    assert!(!legacy.dirty);
    assert_eq!(before, f.task());
    assert!(inspect(&f.ctx, &f.id, "R99").is_err());
    f.register(false); // Even optional unaudited criteria get canonical inspection, not an empty PASS.
    assert_eq!(f.inspect().criteria[0].failures, ["evidence_missing:C1"]);
    f.record(ObservationStatus::Pass);
    let inspection = f.inspect();
    assert!(inspection.criteria[0].failures.is_empty());
    assert_eq!(
        inspection.criteria[0]
            .observation
            .as_ref()
            .unwrap()
            .authority,
        "worker_assertion"
    );
    assert!(!inspection.criteria[0].implemented);
    assert!(inspection.review.is_none());
}

#[test]
fn changed_candidate_manifest_and_snapshot_bytes_invalidate_proof_in_the_canonical_adapter() {
    for defect in ["candidate", "manifest", "artifact", "artifact_identity"] {
        let f = Fixture::new();
        f.register(true);
        f.record(ObservationStatus::Pass);
        assert!(f.inspect().criteria[0].failures.is_empty());
        match defect {
            "candidate" => {
                std::fs::write(f.root.join("code.txt"), "new candidate\n").unwrap();
                corral_git::commit_all(&f.root, "new candidate").unwrap();
            }
            "manifest" => {
                f.ctx
                    .store
                    .update(&f.id, |t| {
                        t.completion.as_mut().unwrap().criteria[0].expected =
                            "changed expectation".into();
                        Ok(())
                    })
                    .unwrap();
            }
            "artifact_identity" => {
                f.ctx
                    .store
                    .update(&f.id, |t| {
                        t.artifacts[0].sha256 = "f".repeat(64);
                        Ok(())
                    })
                    .unwrap();
            }
            _ => {
                std::fs::write(f.ctx.store.dir().join(&f.artifact), "tampered\n").unwrap();
            }
        }
        let inspection = f.inspect();
        let expected = if defect.starts_with("artifact") {
            "evidence_artifact_invalid:C1"
        } else {
            "evidence_stale:C1"
        };
        assert!(
            inspection.criteria[0]
                .failures
                .iter()
                .any(|s| s == expected),
            "{defect}: {inspection:?}"
        );
        if defect == "manifest" {
            assert!(
                inspection
                    .manifest_failures
                    .contains(&"run_manifest_stale".into())
            );
        }
    }
}

#[test]
fn failed_and_not_run_observations_remain_failed_and_not_run() {
    for (status, failure) in [
        (ObservationStatus::Fail, "evidence_failed:C1"),
        (ObservationStatus::NotRun, "evidence_not_run:C1"),
    ] {
        let f = Fixture::new();
        f.register(true);
        f.record(status);
        assert!(f.inspect().criteria[0].failures.contains(&failure.into()));
    }
}

#[test]
fn failed_gate_legacy_approval_and_superseded_findings_never_become_criterion_acceptance() {
    let f = Fixture::new();
    f.register(true);
    f.record(ObservationStatus::Pass);
    f.ctx.store.update(&f.id, |t| {
        let run = t.current_run_mut().unwrap();
        let review: corral_core::ReviewRecord = serde_json::from_value(serde_json::json!({
            "reviewer":"owner", "verdict":"approve", "notes":"Owner review notes", "sha":corral_git::rev_sha(&f.root, "HEAD").unwrap(), "at":"2026-01-03T00:00:00Z"
        })).unwrap();
        run.review = Some(review.clone());
        let mut child = run.clone(); child.id = "RV1".into(); child.outcome = Some(RunOutcome::Superseded);
        child.review.as_mut().unwrap().verdict = Some(Verdict::Changes);
        run.review_runs.push(child);
        run.gate = Some(serde_json::from_value(serde_json::json!({
            "sha": review.sha, "commands":[], "result":"fail", "at":"2026-01-01T00:00:00Z"
        })).unwrap());
        Ok(())
    }).unwrap();
    let result = f.inspect();
    assert_eq!(result.gate.as_ref().unwrap().result, GateResult::Fail);
    assert!(!result.gate.as_ref().unwrap().fresh);
    assert_eq!(
        result.review.as_ref().unwrap().verdict,
        Some(Verdict::Approve)
    );
    assert!(!result.review.as_ref().unwrap().fresh);
    assert_eq!(
        result.review.as_ref().unwrap().notes.as_deref(),
        Some("Owner review notes")
    );
    assert!(result.findings[0].superseded);
    assert_eq!(result.findings[0].verdict, Some(Verdict::Changes));
    assert!(
        serde_json::to_value(result)
            .unwrap()
            .get("accepted")
            .is_none()
    );
}

#[test]
fn oversize_inputs_are_unavailable_not_empty_success() {
    let f = Fixture::new();
    f.register(true);
    f.ctx
        .store
        .update(&f.id, |t| {
            let manifest = t.completion.as_mut().unwrap();
            manifest.criteria = vec![manifest.criteria[0].clone(); MAX_CRITERIA + 1];
            Ok(())
        })
        .unwrap();
    assert!(
        inspect(&f.ctx, &f.id, "R1")
            .unwrap_err()
            .to_string()
            .contains("inspection_limit")
    );
}
