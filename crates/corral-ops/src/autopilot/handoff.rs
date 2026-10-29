//! The autopilot's handoffs to the owner: escalation to Needs me, and a blocked landing.
use anyhow::Result;
use corral_core::{AutoStage, AutopilotState, Status};

use super::{Change, Pilot, Step};
use crate::cleanup::{Cleanup, cleanup_head};
use crate::gate::run_head_sha;

impl Pilot<'_> {
    /// Hands the task to the owner: Needs me with the reason, a notification, one event.
    pub(super) fn escalate(&self, reason: &str) -> Result<Step> {
        let from = self.stage();
        let state = AutopilotState {
            stage: AutoStage::Escalated,
            reason: Some(reason.to_string()),
            sha: run_head_sha(self.run),
            at: self.now,
        };
        if !self.write(from, state, Status::NeedsMe)? {
            return Ok(Step::Wait);
        }
        cleanup_head(self.ctx, &self.ctx.store.head(self.id())?, Cleanup::Round);
        if let Err(e) = self.ctx.herdr.notify(
            &format!("▲ {} needs you", self.id()),
            &format!("autopilot: {reason}"),
            true,
        ) {
            crate::messages::report(
                Some(self.id()),
                "notification",
                format!("notification for {} failed: {e:#}", self.id()),
            );
        }
        self.log(
            AutoStage::Escalated,
            self.head.rounds,
            None,
            Some(reason),
            "handed to the owner",
        )?;
        Ok(Step::Moved(Change {
            task: self.head.id.clone(),
            from: Status::Review,
            to: Status::NeedsMe,
            note: format!("autopilot escalated: {reason}"),
            stage: Some(AutoStage::Escalated),
        }))
    }

    /// Landing waits for `reason` to clear; later syncs retry. Notifies once, on entering Blocked.
    pub(super) fn block(&self, reason: &str) -> Result<Step> {
        if let Some(state) = self.state().filter(|a| a.stage == AutoStage::Blocked) {
            if state.reason.as_deref() != Some(reason) {
                // A different reason for the same wait: refresh it, no new transition.
                let refreshed = AutopilotState {
                    reason: Some(reason.to_string()),
                    ..state.clone()
                };
                self.write(Some(AutoStage::Blocked), refreshed, Status::Review)?;
            }
            return Ok(Step::Wait);
        }
        let sha = self.state().and_then(|a| a.sha.clone());
        let step = self.moved(
            AutoStage::Blocked,
            sha,
            Some(reason),
            "landing blocked; retried on later syncs",
        )?;
        if matches!(step, Step::Moved(_))
            && let Err(e) =
                self.ctx
                    .herdr
                    .notify(&format!("⏸ {} cannot land", self.id()), reason, false)
        {
            crate::messages::report(
                Some(self.id()),
                "notification",
                format!("notification for {} failed: {e:#}", self.id()),
            );
        }
        Ok(step)
    }
}
