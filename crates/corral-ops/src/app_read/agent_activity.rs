//! Facts derived from the existing bounded transcript window, never a history scan.
use corral_core::{RunProgress, StepState};
use corral_protocol::rows::{
    AGENT_ACTIVITY_MINUTES, AGENT_STEPS_MAX, AgentActivity, AgentRow, AgentStep, AgentStepState,
};
use jiff::Timestamp;

use super::short;

#[derive(Default)]
pub(super) struct Output {
    pub lines: Vec<String>,
    pub last_output_at: Option<String>,
    pub activity: Option<AgentActivity>,
}

impl Output {
    pub fn apply(self, row: &mut AgentRow) {
        row.tail = self.lines;
        // A gate's attempt time remains known even if its scratch log is unavailable.
        if self.last_output_at.is_some() {
            row.last_output_at = self.last_output_at;
        }
        row.activity = self.activity;
    }
}

/// Counts complete timestamped records from the existing 128 KiB tail window. Busy sessions
/// can undercount older minutes when that window covers less than twenty minutes. Counts
/// saturate at the wire's u16 bound; no second read or history walk fills missing records.
///
/// The clock is rounded down to a minute. Empty windows omit activity, including `until`,
/// so quiet rows remain byte-identical after their last event ages out. A recent window
/// can age once per minute, never on each daemon poll. The Agents publisher compares full
/// rows (including their content-derived rev) and therefore does not publish empty ticks.
pub(super) struct Records {
    now: Timestamp,
    minute: i64,
    counts: [u16; AGENT_ACTIVITY_MINUTES],
    newest: Option<Timestamp>,
}

impl Records {
    pub fn new(now: Timestamp) -> Self {
        Self {
            now,
            minute: now.as_second().div_euclid(60),
            counts: [0; AGENT_ACTIVITY_MINUTES],
            newest: None,
        }
    }

    pub fn observe(&mut self, at: Timestamp) {
        if at > self.now {
            return;
        }
        self.newest = Some(self.newest.map_or(at, |previous| previous.max(at)));
        let age = self.minute - at.as_second().div_euclid(60);
        if (0..AGENT_ACTIVITY_MINUTES as i64).contains(&age) {
            let bucket = &mut self.counts[AGENT_ACTIVITY_MINUTES - 1 - age as usize];
            *bucket = bucket.saturating_add(1);
        }
    }

    pub fn finish(self, lines: Vec<String>) -> Output {
        let activity = if self.counts.iter().any(|count| *count != 0) {
            Timestamp::from_second(self.minute * 60)
                .ok()
                .map(|until| AgentActivity {
                    until: until.to_string(),
                    per_minute: self.counts.to_vec(),
                })
        } else {
            None
        };
        Output {
            lines,
            last_output_at: self.newest.map(|at| at.to_string()),
            activity,
        }
    }
}

pub(super) fn steps(progress: Option<&RunProgress>) -> Option<Vec<AgentStep>> {
    let steps: Vec<_> = progress
        .into_iter()
        .flat_map(RunProgress::steps)
        .take(AGENT_STEPS_MAX)
        .map(|step| AgentStep {
            title: short(&step.label, 120),
            state: match step.status {
                StepState::Pending => AgentStepState::Pending,
                StepState::InProgress => AgentStepState::InProgress,
                StepState::Done => AgentStepState::Done,
                StepState::Dropped { .. } => AgentStepState::Dropped,
            },
        })
        .collect();
    (!steps.is_empty()).then_some(steps)
}

#[cfg(test)]
#[path = "agent_activity_tests.rs"]
mod tests;
