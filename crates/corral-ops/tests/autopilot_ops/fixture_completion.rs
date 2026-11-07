//! Shared completion/attribution fixture operations, kept below the test-module ceiling.
use super::*;
impl Fx {
    /// Explicit semantic input for fixtures that model a genuine candidate defect rather
    /// than an unknown/environment failure. Receipt collection alone never attributes it.
    pub(super) fn attribute_candidate_failure(&self) {
        let failed = self.run().gate.unwrap();
        let proof = self.bin_dir.join("candidate-diagnosis.md");
        std::fs::write(
            &proof,
            format!(
                "Fixture candidate defect at {}: {:?}",
                failed.sha, failed.failed_cmd
            ),
        )
        .unwrap();
        let artifact = corral_ops::add_artifact(
            &self.ctx,
            &self.id,
            &proof,
            Some(corral_core::ArtifactKind::Report),
            Some("candidate diagnosis"),
        )
        .unwrap();
        corral_ops::gate_triage::diagnose(
            &self.ctx,
            &self.id,
            corral_ops::gate_triage::GateDiagnosis {
                attempt: failed.identity.unwrap().attempt,
                class: corral_ops::gate_triage::FailureClass::Candidate,
                explanation:
                    "Precisely attributed synthetic candidate defect for repair-policy testing"
                        .into(),
                evidence: vec![artifact.path],
                baseline: None,
                retry: false,
            },
            true,
        )
        .unwrap();
    }

    pub(super) fn done(&self) {
        self.done_with("made the change");
    }

    pub(super) fn done_with(&self, summary: &str) {
        let task = self.task();
        let run = task.current_run().unwrap();
        corral_git::commit_all(
            &run.worktree,
            &format!("{}: {} (corral run {})", task.id, task.title, run.id),
        )
        .unwrap();
        let identity = corral_ops::progress_readback(&self.ctx, &self.id).unwrap();
        corral_ops::save_checkpoint(
            &self.ctx,
            &self.id,
            &corral_core::CheckpointUpdate {
                task: identity.task,
                run: identity.run,
                spec_revision: identity.spec_revision,
                expected_revision: identity
                    .checkpoints
                    .last()
                    .map_or(0, |r| r.checkpoint.revision),
                progress_revision: identity.progress.as_ref().map_or(0, |p| p.revision),
                head: identity.head,
                base: identity.base,
                tree: identity.tree,
                criterion_ids: vec![],
                evidence_refs: vec![],
                next_action: "Review the candidate".into(),
                decisions: vec![],
                waits: vec![],
                resources: vec![],
            },
        )
        .unwrap();
        corral_ops::done(&self.ctx, &self.id, summary).unwrap();
    }
}
