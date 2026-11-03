//! Slot-holding and candidate predicates of execution records.
use super::{GateRecord, GateResult, ReviewRecord};

impl GateRecord {
    pub fn holds_slot(&self) -> bool {
        self.result == GateResult::Running && !self.process_lost
    }

    /// True when the gate passed on exactly `head`.
    pub fn passed_at(&self, head: &str) -> bool {
        self.result == GateResult::Pass && self.sha == head
    }
}

impl ReviewRecord {
    pub fn holds_slot(&self) -> bool {
        self.verdict.is_none() && !self.process_lost && !self.cancelled
    }
}
