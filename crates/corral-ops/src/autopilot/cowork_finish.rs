//! Co-work finish checks and repair instructions kept out of the shared autopilot driver.
use corral_core::{Accept, TaskFieldsRef as _};

use super::*;

impl Pilot<'_> {
    pub(super) fn cowork_initial_step(&self) -> Result<Option<Step>> {
        if self.head.cowork_run().is_some()
            && !matches!(self.stage(), Some(AutoStage::Escalated | AutoStage::Done))
        {
            if let Some(step) = self.cowork_wait()? {
                return Ok(Some(step));
            }
            // A failed co-work gate already ran at finish. Do not silently retry it, even if
            // main moved: this candidate's defect belongs to its originating assistant.
            if let Some(gate) = self.run.gate.as_ref().filter(|gate| {
                gate.result == GateResult::Fail
                    && run_head_sha(self.run).as_deref() == Some(&gate.sha)
            }) {
                if self.require(OperatingActionKind::Repair).is_err() {
                    return Ok(Some(Step::Wait));
                }
                let (summary, details) = self.gate_failure(gate, "")?;
                return self.request_changes(&summary, &details).map(Some);
            }
        }
        Ok(None)
    }

    pub(super) fn owner_acceptance_required(&self) -> bool {
        !self
            .head
            .cowork_run()
            .is_some_and(|cowork| cowork.owner_confirmed)
            && self.head.review == Some(Accept::Owner)
            && self
                .config
                .review_policy(self.head.project.as_deref(), None)
                .accept
                == Accept::Auto
    }

    pub(super) fn repair_finish_instruction(&self, corral: &str) -> String {
        let id = self.id();
        if let Some(cowork) = self.head.cowork_run() {
            format!(
                "Fix and commit in assistant {}; resubmit scratch {} with the original owner finish instruction and `--into {id} --expected-head <new HEAD>`.",
                cowork.session, cowork.scratch
            )
        } else {
            format!(
                "Fix it, commit, and run `{corral} done {id} --summary \"<what changed>\"` again."
            )
        }
    }
}
