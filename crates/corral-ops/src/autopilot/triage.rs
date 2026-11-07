//! Failure attribution precedes any charged code repair, for every completion driver.
use super::*;

impl Pilot<'_> {
    pub(super) fn gate_triage_step(&self) -> Result<Option<Step>> {
        if matches!(
            self.stage(),
            Some(AutoStage::Escalated | AutoStage::Done | AutoStage::Changes)
        ) {
            return Ok(None);
        }
        let Some(gate) = self.run.gate.as_ref() else {
            return Ok(None);
        };
        if gate.result == GateResult::Pass {
            if let Err(error) =
                crate::gate_triage::require_pass(&self.ctx.store, self.id(), Some(gate))
            {
                return self.escalate(&error.to_string()).map(Some);
            }
            return Ok(None);
        }
        if gate.result != GateResult::Fail || run_head_sha(self.run).as_deref() != Some(&gate.sha) {
            return Ok(None);
        }
        if self.require(OperatingActionKind::Gate).is_err() {
            return Ok(Some(Step::Wait));
        }
        self.triage_failure(gate).map(Some)
    }

    pub(super) fn triage_failure(&self, gate: &GateRecord) -> Result<Step> {
        match crate::gate_triage::route(&self.ctx.store, self.id(), gate)? {
            crate::gate_triage::Route::Repair => {
                let (summary, details) = self.gate_failure(gate, "")?;
                self.request_changes(&summary, &details)
            }
            crate::gate_triage::Route::Retry => {
                let Some(head) = run_head_sha(self.run) else {
                    return self.escalate("candidate checkout missing");
                };
                match self.start_gate(&head)? {
                    GateNow::Replayed(step) => Ok(step),
                    _ => Ok(Step::Acted),
                }
            }
            crate::gate_triage::Route::Block(reason, diagnose) => {
                let (_, details) = self.gate_failure(gate, "")?;
                let step = self.escalate(&format!("{reason}\n\n{details}"))?;
                if diagnose
                    && matches!(step, Step::Moved(_))
                    && let Err(error) = self.queue_gate_diagnosis(gate)
                {
                    crate::messages::report(
                        Some(self.id()),
                        "cowork",
                        format!(
                            "diagnosis remains blocked; assistant notification deferred: {error:#}"
                        ),
                    );
                }
                Ok(step)
            }
        }
    }
}
