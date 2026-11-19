//! Land's steps as its host runs them (`ExitBrief.land`): one row per host step in run order,
//! labelled by its id, in the host's state with its note and time, and the gate's log under the
//! gate step while the gate runs or after it failed. The dock's exit progress and the finish
//! card both draw these rows; a host that reports no `land` leaves them deriving the rail from
//! the phase.

use corral_protocol::session_state::{LandBrief, LandGate, LandStep, LandStepId, LandStepState};

use crate::model::agents::unix;

use super::activity::duration;
use super::exit::StepState;
use super::transcript::{Line, LineClass, line};

/// One step of Land as the host reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandRow {
    /// `Rebase onto main`, `Full gate`, `Land (fast-forward main)`, `Step`.
    pub label: String,
    pub state: StepState,
    /// The host's fact: `3f2a9c1`, `G4 passed`, `stopped: main moved`.
    pub note: Option<String>,
    /// `3m 12s`: running to now, or from start to end.
    pub time: Option<String>,
    pub log: Option<GateLog>,
}

/// The gate's log under its step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateLog {
    /// `scripts/gate.sh · 1 of 2`, over the tail.
    pub head: Option<String>,
    /// The log's last lines, oldest first.
    pub lines: Vec<Line>,
    /// `log: /home/…/gate.log`.
    pub path: Option<String>,
}

/// The host's steps of `land` onto `base`; `now` is the feed's clock in epoch seconds.
pub(super) fn rows(land: &LandBrief, base: &str, now: u64) -> Vec<LandRow> {
    let mut logged = false;
    land.steps
        .iter()
        .map(|step| {
            let state = match step.state {
                LandStepState::Done => StepState::Done,
                LandStepState::Running => StepState::Running,
                LandStepState::Failed => StepState::Failed,
                LandStepState::Waiting | LandStepState::Other => StepState::Waiting,
            };
            let label = match step.id {
                LandStepId::Rebase => format!("Rebase onto {base}"),
                LandStepId::Gate => "Full gate".to_owned(),
                LandStepId::Land => format!("Land (fast-forward {base})"),
                LandStepId::Other => "Step".to_owned(),
            };
            let log = (step.id == LandStepId::Gate && !logged)
                .then(|| gate_log(land.gate.as_ref()?, state))
                .flatten();
            logged |= step.id == LandStepId::Gate;
            LandRow {
                label,
                state,
                note: step.note.clone().filter(|note| !note.trim().is_empty()),
                time: time(step, now),
                log,
            }
        })
        .collect()
}

/// A running step's time to `now`, a settled one's from start to end; none under a second or
/// when a time does not parse.
fn time(step: &LandStep, now: u64) -> Option<String> {
    let start = unix(step.started_at.as_deref()?)?;
    let end = match step.state {
        LandStepState::Running => Some(now).filter(|now| *now > 0)?,
        LandStepState::Done | LandStepState::Failed => unix(step.ended_at.as_deref()?)?,
        LandStepState::Waiting | LandStepState::Other => return None,
    };
    let seconds = end.checked_sub(start)?;
    (seconds > 0).then(|| duration(seconds))
}

/// The tail with its command whenever the host sends one; the log's path alone while the gate
/// runs or after it failed without a tail.
fn gate_log(gate: &LandGate, state: StepState) -> Option<GateLog> {
    let live = matches!(state, StepState::Running | StepState::Failed);
    if gate.tail.is_empty() && !(live && gate.log.is_some()) {
        return None;
    }
    let head = gate
        .command
        .clone()
        .filter(|_| !gate.tail.is_empty())
        .map(|command| match (gate.index, gate.of) {
            (Some(index), Some(of)) if of > 1 => format!("{command} · {index} of {of}"),
            _ => command,
        });
    Some(GateLog {
        head,
        lines: gate
            .tail
            .iter()
            .map(|said| line(said, LineClass::Dim))
            .collect(),
        path: gate.log.as_ref().map(|path| format!("log: {path}")),
    })
}
