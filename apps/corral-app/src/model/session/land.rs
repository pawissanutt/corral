//! Land's steps as its host runs them (`ExitBrief.land`): one row per host step in run order,
//! labelled by its id, in the host's state with its note and time, and the gate's log under the
//! gate step while the gate runs or after it failed. The dock's exit progress and the finish
//! card both draw these rows; a host that reports no `land` leaves them deriving the rail from
//! the phase.
//!
//! The candidate the host prepared (`ExitBrief.candidate`): the exact base commit and head Land
//! would fast-forward between, or what holds it back: conflicts, a missing review or gate, a
//! failed gate, a base that moved. A host that prepared none reports none, and nothing here
//! claims one.

use corral_protocol::session_state::{
    LandBrief, LandCandidate, LandCandidateState, LandGate, LandStep, LandStepId, LandStepState,
};

use crate::model::agents::unix;
use crate::model::hue::Hue;

use super::activity::duration;
use super::exit::StepState;
use super::transcript::{Line, LineClass, line};

/// Conflicted paths listed under a candidate; the rest are counted.
const CONFLICTS_SHOWN: usize = 5;

/// The candidate the host prepared, as the owner reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateView {
    pub state: LandCandidateState,
    /// `✓`, `✗`, `▲`, `·`; none while it is prepared (the spinner).
    pub glyph: Option<&'static str>,
    pub hue: Hue,
    /// `Ready: Land fast-forwards main 1a2b3c4 → 5d6e7f8`,
    /// `Rebasing onto main conflicts; the branch is unchanged`.
    pub text: String,
    /// Muted after the text: `gate G4 passed`, `2 paths`.
    pub facts: Option<String>,
    /// The conflicted paths, then `and 3 more`.
    pub paths: Vec<String>,
    /// The host's reason; a ready candidate needs none.
    pub detail: Option<String>,
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// `main 1a2b3c4 → 5d6e7f8`: the fast-forward `candidate` stands for; none without both commits.
pub fn fast_forward(candidate: &LandCandidate) -> Option<String> {
    let tip = candidate.base_tip.as_deref()?;
    let head = candidate.head.as_deref()?;
    let base = candidate.base.as_deref().unwrap_or("the base branch");
    Some(format!("{base} {} → {}", short(tip), short(head)))
}

/// Where the host's candidate stands, in its own words where it gives them.
pub fn candidate(candidate: &LandCandidate) -> CandidateView {
    use LandCandidateState as S;
    let base = candidate.base.as_deref().unwrap_or("the base branch");
    let on = candidate
        .head
        .as_deref()
        .map_or_else(String::new, |head| format!(" {}", short(head)));
    let gate = candidate.gate_attempt.as_deref();
    let ff = fast_forward(candidate);
    let (glyph, hue, text, facts) = match candidate.state {
        S::Preparing => (
            None,
            Hue::Corral,
            format!("Preparing the Land candidate on {base}"),
            None,
        ),
        S::Ready => (
            Some("✓"),
            Hue::Ok,
            ff.as_ref().map_or_else(
                || format!("Candidate{on} ready on {base}"),
                |ff| format!("Ready: Land fast-forwards {ff}"),
            ),
            gate.map(|gate| format!("gate {gate} passed")),
        ),
        S::Conflicts => (
            Some("✗"),
            Hue::Fail,
            format!("Rebasing onto {base} conflicts; the branch is unchanged"),
            match candidate.conflicts.len() {
                0 => None,
                1 => Some("1 path".to_owned()),
                n => Some(format!("{n} paths")),
            },
        ),
        S::NeedsReview => (
            Some("▲"),
            Hue::Held,
            format!("Candidate{on} on {base} needs an approving review"),
            ff.clone(),
        ),
        S::NeedsGate => (
            Some("▲"),
            Hue::Held,
            format!("Candidate{on} on {base} has no passing gate"),
            ff.clone(),
        ),
        S::GateFailed => (
            Some("✗"),
            Hue::Fail,
            format!(
                "Gate{} failed on candidate{on}",
                gate.map_or_else(String::new, |gate| format!(" {gate}"))
            ),
            None,
        ),
        S::BaseMoved => (
            Some("▲"),
            Hue::Held,
            format!(
                "{base} moved{} since the candidate was prepared{}",
                candidate
                    .moved_to
                    .as_deref()
                    .map_or_else(String::new, |to| format!(" to {}", short(to))),
                candidate
                    .base_tip
                    .as_deref()
                    .map_or_else(String::new, |tip| format!(" on {}", short(tip))),
            ),
            None,
        ),
        S::Failed => (
            Some("✗"),
            Hue::Fail,
            "Preparing the Land candidate failed".to_owned(),
            None,
        ),
        S::Other => (
            Some("·"),
            Hue::Neutral,
            format!("Land candidate{on} on {base}"),
            None,
        ),
    };
    let mut paths: Vec<String> = Vec::new();
    if candidate.state == S::Conflicts {
        paths.extend(candidate.conflicts.iter().take(CONFLICTS_SHOWN).cloned());
        if candidate.conflicts.len() > CONFLICTS_SHOWN {
            paths.push(format!(
                "and {} more",
                candidate.conflicts.len() - CONFLICTS_SHOWN
            ));
        }
    }
    CandidateView {
        state: candidate.state,
        glyph,
        hue,
        text,
        facts,
        paths,
        detail: candidate
            .detail
            .clone()
            .filter(|detail| candidate.state != S::Ready && !detail.trim().is_empty()),
    }
}

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
    rows_with(land, base, now, None)
}

/// [`rows`], with the fast-forward naming the commits it moves between when the host
/// prepared `candidate`.
pub(super) fn rows_with(
    land: &LandBrief,
    base: &str,
    now: u64,
    candidate: Option<&LandCandidate>,
) -> Vec<LandRow> {
    let target = candidate
        .and_then(fast_forward)
        .unwrap_or_else(|| base.to_owned());
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
                LandStepId::Land => format!("Land (fast-forward {target})"),
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
