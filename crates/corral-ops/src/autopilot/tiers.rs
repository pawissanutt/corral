//! Gate tiers in the autopilot: landing stages gate the landing tier (full on main by constant),
//! a candidate behind its destination is replayed before any gate starts on it, and a tiered
//! approval is promoted onto its full landing receipt before the evidence check.
use super::*;

impl Pilot<'_> {
    /// Landing gates the landing tier even before an approval exists (reviewer = none).
    fn landing_phase(&self) -> bool {
        matches!(self.stage(), Some(AutoStage::Landing | AutoStage::Blocked))
    }

    /// Whether `gate` stands for this stage: the landing requirement while landing, else the
    /// requirement of the run's current phase.
    pub(super) fn gate_stands(&self, gate: &GateRecord) -> Result<bool> {
        if self.landing_phase() {
            crate::review::landing_gate_fresh(self.ctx, self.task, self.run, gate)
        } else {
            verification::gate_fresh(self.ctx, self.task, self.run, gate)
        }
    }

    /// Starts a gate on `head`, or first replays a candidate that does not contain its
    /// destination head so that the gate runs on the replayed head.
    pub(super) fn start_gate(&self, head: &str) -> Result<GateNow> {
        let base = corral_git::rev_sha(&self.run.worktree, &self.run.base)?;
        if !corral_git::is_ancestor(&self.run.worktree, &base, head)? {
            return self.rebase().map(GateNow::Replayed);
        }
        if self.landing_phase() {
            let origin = crate::operating::Origin::Automatic;
            crate::gate::gate_start_landing(self.ctx, self.id(), origin)?;
        } else {
            crate::gate::gate_start_automatic(self.ctx, self.id())?;
        }
        Ok(GateNow::Started)
    }

    /// Tiered runs carry a quick approval and its evidence onto the full landing receipt.
    pub(super) fn promote(&self) -> Result<bool> {
        if self.run.gate_rules.is_legacy() {
            return Ok(false);
        }
        crate::verdict_carry::promote(self.ctx, self.id())
    }
}
