//! Co-work finish checks and repair instructions kept out of the shared autopilot driver.
use corral_core::{Accept, TaskFieldsRef as _};

use super::*;

impl Pilot<'_> {
    pub(super) fn cowork_initial_step(&self) -> Result<Option<Step>> {
        if self
            .head
            .cowork_run()
            .and_then(|cowork| cowork.completion.as_ref())
            .is_some()
            && self.stage() == Some(AutoStage::Gating)
            && self
                .state()
                .and_then(|state| state.sha.as_deref())
                .is_some_and(|submitted| run_head_sha(self.run).as_deref() != Some(submitted))
        {
            return self
                .escalate("submitted candidate changed before gate admission; resubmit explicitly")
                .map(Some);
        }
        if self.head.cowork_run().is_some()
            && !matches!(self.stage(), Some(AutoStage::Escalated | AutoStage::Done))
            && let Some(step) = self.cowork_wait()?
        {
            return Ok(Some(step));
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
        if let Some((session, scratch)) = self
            .head
            .cowork_run()
            .and_then(|cowork| cowork.assistant_driver())
        {
            format!(
                "Fix and commit in assistant {session}; resubmit scratch {scratch} with the original owner finish instruction and `--into {id} --expected-head <new HEAD>`."
            )
        } else {
            format!(
                "Fix it, commit, and run `{corral} done {id} --summary \"<what changed>\"` again."
            )
        }
    }
}
