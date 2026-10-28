//! Runs parked on another task release their coding slot; red-main fixes take capacity first.
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{Run, RunObligations, Task, WaitHandle, WaitRecord, WaitState};

/// Label that marks a task fixing failing tests on the integration base.
pub const RED_MAIN_LABEL: &str = "red_main";

/// A parked run whose wait fired: it holds no slot until corral grants it one again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotWait {
    pub since: Timestamp,
    /// The tasks the run was parked on when it released its slot.
    pub on: Vec<String>,
}

/// True for a Ready or running task whose purpose is fixing a red integration base.
pub fn red_main(task: &Task) -> bool {
    task.labels
        .iter()
        .any(|l| l == RED_MAIN_LABEL || l == "red-main")
}

/// The task a channel handle names (`task:T-512:landed`, `worker:T-512-landed`), if any.
pub fn awaited_task(handle: &WaitHandle) -> Option<&str> {
    let WaitHandle::Channel { channel_id, .. } = handle else {
        return None;
    };
    channel_id
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .find_map(task_token)
}

fn task_token(segment: &str) -> Option<&str> {
    let rest = segment.strip_prefix("T-")?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let end = 2 + digits;
    (digits >= 3 && (segment.len() == end || segment.as_bytes()[end] == b'-'))
        .then(|| &segment[..end])
}

impl WaitRecord {
    /// Still unresolved: no receipt and no terminal or diagnostic-final observation.
    pub fn open(&self) -> bool {
        self.receipt.is_none()
            && matches!(
                self.observation.state,
                WaitState::Pending | WaitState::Unsupported | WaitState::Unverifiable
            )
    }
}

impl RunObligations {
    /// Tasks this run is parked on: every open wait names another task and no question is pending.
    pub fn parked_on(&self, own: &str) -> Vec<String> {
        if self.pending_decision().is_some() {
            return Vec::new();
        }
        let mut on = Vec::new();
        for wait in self.waits.iter().filter(|w| w.open()) {
            match awaited_task(&wait.request.handle).filter(|task| *task != own) {
                Some(task) if !on.iter().any(|t| t == task) => on.push(task.to_string()),
                Some(_) => {}
                None => return Vec::new(),
            }
        }
        on
    }

    /// Call after any obligation change with the parked set from before it: a parked run that
    /// stops waiting on other tasks queues for a slot instead of taking one back implicitly.
    pub fn settle_parking(&mut self, before: &[String], own: &str, now: Timestamp) {
        if !self.parked_on(own).is_empty() {
            self.slot_wait = None;
        } else if !before.is_empty() && self.slot_wait.is_none() {
            self.slot_wait = Some(SlotWait {
                since: now,
                on: before.to_vec(),
            });
        }
    }
}

impl Run {
    /// Parked on other tasks with no live gate or reviewer.
    pub fn parked_on(&self, own: &str) -> Vec<String> {
        if self.outcome.is_some()
            || self
                .gate
                .as_ref()
                .is_some_and(crate::GateRecord::holds_slot)
            || self
                .review
                .as_ref()
                .is_some_and(crate::ReviewRecord::holds_slot)
        {
            return Vec::new();
        }
        self.obligations.parked_on(own)
    }

    /// The open run holds no coding slot: parked on other tasks, or woken and queued for a slot.
    pub fn releases_slot(&self, own: &str) -> bool {
        self.outcome.is_none()
            && (self.obligations.slot_wait.is_some() || !self.parked_on(own).is_empty())
    }
}

#[cfg(test)]
#[path = "slot_release_tests.rs"]
mod tests;
