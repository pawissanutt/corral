//! Issue one repair cycle to its current driver using the normal executors.
use super::*;
use crate::dispatch::{auto_request_changes, auto_retry};

impl Pilot<'_> {
    /// Sends the run back to its agent with `summary` (one line) and `details`, or escalates when the rounds are
    /// used up. Reuse resumes a missing worker's recorded session; explicit fresh mode or an incompatible
    /// profile escalation starts a retry whose prompt carries the request.
    pub(super) fn send_request(
        &self,
        summary: &str,
        details: &str,
        key: &RepairKey,
        packet: Option<&str>,
    ) -> Result<Step> {
        let _action = corral_core::work::action(self.id(), OperatingActionKind::Repair);
        key.check(&self.ctx.store.head(self.id())?)?;
        let digest = key.digest()?;
        if packet.is_some() && output::requested(self.ctx, key, &digest)? {
            return Ok(Step::Wait);
        }
        let issued = match crate::repair_cycle::load(&self.ctx.store, self.id())? {
            Some(cycle) if cycle.issued && cycle.key.digest()? == digest => Some(cycle),
            _ => None,
        };
        if issued
            .as_ref()
            .is_some_and(|cycle| cycle.driver == key.driver)
            && self.stage() == Some(AutoStage::Changes)
        {
            return Ok(Step::Wait);
        }
        let budget = if issued.is_some() {
            repair::RepairBudget::Legacy
        } else {
            self.repair_budget(&digest, summary)?
        };
        let (bounded, policy_switch, max) = match budget {
            repair::RepairBudget::Legacy => (false, None, self.policy.max_rounds),
            repair::RepairBudget::Coding { switch, max } => (true, switch, max),
            repair::RepairBudget::Stop(step) => return Ok(step),
        };
        let round = issued
            .as_ref()
            .map_or(self.head.rounds + 1, |cycle| cycle.round);
        if !bounded && round > max && !self.ctx.store.config.automation.turbo {
            return self.escalate(&format!("{summary} (all {max} change rounds used)"));
        }
        self.require(OperatingActionKind::Repair)?;
        crate::repair_cycle::prepare(self.ctx, key, round, summary, details)?;
        let id = self.id();
        let corral = corral_bin()?.display().to_string();
        let mut request = format!("{summary}\n");
        if !details.trim().is_empty() {
            request.push_str(&format!("\n{}\n", details.trim_end()));
        }
        let finish = self.repair_finish_instruction(&corral);
        let document =
            format!("# Changes requested: {id} round {round} of {max}\n\n{request}\n{finish}\n");
        let artifact = self.snapshot(
            &format!("changes-round-{round}.md"),
            &document,
            &format!("Autopilot changes (round {round})"),
        )?;
        let file = crate::artifact_path(self.ctx, &artifact)?;
        let message = format!(
            "corral autopilot: changes requested on {id} (round {round} of {max}): {}. Details: {}. {finish}",
            summary.trim_end_matches('.'),
            file.display()
        );
        let state = AutopilotState {
            stage: AutoStage::Changes,
            reason: Some(request),
            sha: run_head_sha(self.run),
            at: self.now,
        };
        if let Some(cowork) = self
            .head
            .cowork_run()
            .filter(|cowork| cowork.assistant_driver().is_some())
        {
            return self.cowork_changes(cowork, &message, summary, state, key, round);
        }
        let pane = self.run.herdr.as_ref().map(|h| h.pane_id.as_str());
        let task = self.ctx.store.head(id)?;
        let switch = if bounded {
            policy_switch
        } else {
            crate::escalation::escalation_head(self.config, &task, false)?
        };
        let reuse = self.config.task_review_policy(&task).worker_session
            == corral_core::SessionPolicy::Reuse;
        let alive = if switch.is_none() {
            self.pane_alive(pane)?
        } else {
            false
        };
        // A board-owned source candidate is not a historical assistant's worker session.
        // Normal retry/adoption and dispatch policy own its continuation.
        let board_source = self.head.cowork_run().is_some_and(|cowork| {
            cowork.assistant_driver().is_none()
                && !cowork.has_worker(&self.run.id, self.run.herdr.is_some())
        });
        let sent: Result<(Status, String)> = if !board_source
            && switch.is_none()
            && (reuse || alive)
        {
            auto_request_changes(self.ctx, id, &message, round, state, key).map(|_| {
                (
                    Status::Running,
                    format!("changes requested (round {round}/{max}): {summary}"),
                )
            })
        } else {
            auto_retry(self.ctx, id, round, state, key).map(|outcome| match outcome {
                DispatchOutcome::Started { run_id, .. } => (
                    Status::Running,
                    format!(
                        "fresh retry run {run_id} carries the change request (round {round}/{max}): {summary}"
                    ),
                ),
                DispatchOutcome::Queued { reason, .. } => (
                    Status::Ready,
                    format!(
                        "fresh retry queued ({reason}) with the change request (round {round}/{max}): {summary}"
                    ),
                ),
            })
        };
        // Capacity or a moved snapshot only delays the send-back: the prepared authorization
        // stays claimed, so a later sync retries the same request instead of handing it to the owner.
        let (to, note) = match sent {
            Err(error) if crate::operating::transient_refusal(&error) => return Ok(Step::Wait),
            Err(error) if error.is::<crate::role_session::SessionUnavailable>() => {
                return self.escalate(&error.to_string());
            }
            sent => sent?,
        };
        if let Some(packet) = packet {
            self.ctx.store.log_event(
                Some(id),
                "integration_request",
                json!({ "run": key.run, "key": key, "digest": digest, "packet": packet }),
            )?;
        }
        self.log(AutoStage::Changes, round, None, Some(summary), &note)?;
        Ok(Step::Moved(Change {
            task: id.to_string(),
            from: Status::Review,
            to,
            note: format!("autopilot {note}"),
            stage: Some(AutoStage::Changes),
        }))
    }
}
