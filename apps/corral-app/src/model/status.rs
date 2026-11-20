//! The engine's words and their marks: a task's eight statuses, a session's four states and
//! the standing of a pipeline stage. Colour says state only and every mark also differs by
//! shape, so a row reads without colour and in a terminal.

use corral_protocol::rows::{AssistantRow, AssistantState};
use corral_types::Status;

use crate::model::hue::Hue;

/// The wait code corrald gives a live Pi host whose status read was refused (an older host's
/// oversized `get_state`).
const STATUS_UNAVAILABLE: &str = "status_unavailable";

/// The host is live but its status could not be read, so nothing says whether it works,
/// waits on the owner or is idle. Other rows in state `Other` are unknown to this app and
/// keep their usual reading.
pub fn status_unavailable(row: &AssistantRow) -> bool {
    row.state == AssistantState::Other
        && row
            .wait
            .as_ref()
            .is_some_and(|wait| wait.code == STATUS_UNAVAILABLE)
}

/// A session's state as the engine's `SessionStatus` names it: Blocked waits on the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SessionState {
    Blocked,
    Working,
    Idle,
    /// Its host is not running: stopped, ended or lost.
    Gone,
}

impl SessionState {
    /// The states a session list groups by, in order; Gone folds at the bottom.
    pub const ORDER: [SessionState; 4] = [Self::Blocked, Self::Working, Self::Idle, Self::Gone];

    pub const fn word(self) -> &'static str {
        match self {
            Self::Blocked => "Blocked",
            Self::Working => "Working",
            Self::Idle => "Idle",
            Self::Gone => "Gone",
        }
    }

    /// The identifier slug (`section-<slug>`).
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Gone => "gone",
        }
    }

    /// What a section of the state says beside its count.
    pub const fn note(self) -> Option<&'static str> {
        match self {
            Self::Blocked => Some("waits on you"),
            Self::Idle => Some("host running, nothing to do"),
            Self::Working | Self::Gone => None,
        }
    }

    /// Whether the state's section folds: Gone, as Done and Dropped do for tasks.
    pub const fn folds(self) -> bool {
        matches!(self, Self::Gone)
    }
}

/// The statuses a task list groups by, in the order the owner reads them: what waits on them
/// first. Done and Dropped ([`CLOSED`]) fold at the bottom.
pub const OPEN: [Status; 6] = [
    Status::Blocked,
    Status::Review,
    Status::Running,
    Status::Ready,
    Status::Backlog,
    Status::Idea,
];

/// The status as the engine spells it, capitalised: `Blocked`.
pub const fn status_word(status: Status) -> &'static str {
    match status {
        Status::Idea => "Idea",
        Status::Backlog => "Backlog",
        Status::Ready => "Ready",
        Status::Running => "Running",
        Status::Blocked => "Blocked",
        Status::Review => "Review",
        Status::Done => "Done",
        Status::Dropped => "Dropped",
    }
}

/// Whether the status's section folds.
pub const fn folds(status: Status) -> bool {
    matches!(status, Status::Done | Status::Dropped)
}

/// Where one of run, gate, review and land stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Passed, finished, landed.
    Ok,
    /// Corral is at it: the gate runs, the reviewer works.
    Now,
    /// A worker is at it.
    Working,
    Fail,
    /// It waits on the owner.
    You,
    /// Its turn has not come.
    Wait,
    /// Not part of where the task is.
    None,
}

impl Stage {
    /// What the mark says to a screen reader, after the stage's name.
    pub const fn word(self) -> &'static str {
        match self {
            Self::Ok => "passed",
            Self::Now | Self::Working => "running",
            Self::Fail => "failed",
            Self::You => "waits on you",
            Self::Wait => "not yet",
            Self::None => "not applicable",
        }
    }
}

/// The shape of a mark, which differs for every state a colour also tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    /// A dashed ring.
    Dashed,
    Ring,
    /// A ring around a dot.
    RingDot,
    /// A half-filled ring.
    Half,
    /// A filled disc.
    Solid,
    /// A ring in a ring.
    RingRing,
    Check,
    Cross,
    Dash,
    Dot,
}

/// How a status, a state or a stage is marked: its shape, its one-Cell glyph for a terminal,
/// its colour, and whether a neutral one recedes (ink-4) rather than reads as ink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub shape: Shape,
    pub glyph: char,
    pub hue: Hue,
    pub quiet: bool,
}

const fn look(shape: Shape, glyph: char, hue: Hue, quiet: bool) -> Look {
    Look {
        shape,
        glyph,
        hue,
        quiet,
    }
}

pub const fn status_look(status: Status) -> Look {
    match status {
        Status::Idea => look(Shape::Dashed, '◌', Hue::Neutral, true),
        Status::Backlog => look(Shape::Ring, '○', Hue::Neutral, true),
        Status::Ready => look(Shape::RingDot, '●', Hue::Neutral, false),
        Status::Running => look(Shape::Half, '◐', Hue::Worker, false),
        Status::Blocked => look(Shape::Solid, '▲', Hue::You, false),
        Status::Review => look(Shape::RingRing, '◆', Hue::Corral, false),
        Status::Done => look(Shape::Check, '✓', Hue::Ok, false),
        Status::Dropped => look(Shape::Cross, '✗', Hue::Neutral, true),
    }
}

pub const fn session_look(state: SessionState) -> Look {
    match state {
        SessionState::Blocked => look(Shape::Solid, '▲', Hue::You, false),
        SessionState::Working => look(Shape::Half, '◐', Hue::Worker, false),
        SessionState::Idle => look(Shape::Ring, '○', Hue::Neutral, true),
        SessionState::Gone => look(Shape::Dash, '–', Hue::Neutral, true),
    }
}

pub const fn stage_look(stage: Stage) -> Look {
    match stage {
        Stage::Ok => look(Shape::Check, '✓', Hue::Ok, false),
        Stage::Now => look(Shape::Half, '◐', Hue::Corral, false),
        Stage::Working => look(Shape::Half, '◐', Hue::Worker, false),
        Stage::Fail => look(Shape::Cross, '✗', Hue::Fail, false),
        Stage::You => look(Shape::Solid, '▲', Hue::You, false),
        Stage::Wait => look(Shape::Ring, '○', Hue::Neutral, true),
        Stage::None => look(Shape::Dot, '·', Hue::Neutral, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without colour (a monochrome terminal, a colour-blind reader) two open statuses that
    /// shared a glyph would read as one state.
    #[test]
    fn every_status_and_state_differs_by_shape_as_well_as_colour() {
        use std::collections::HashSet;
        let closed = [Status::Done, Status::Dropped];
        let statuses = OPEN.into_iter().chain(closed).map(status_look);
        let glyphs: HashSet<char> = statuses.clone().map(|look| look.glyph).collect();
        let shapes: HashSet<Shape> = statuses.map(|look| look.shape).collect();
        assert_eq!((glyphs.len(), shapes.len()), (8, 8));
        let states: HashSet<char> = SessionState::ORDER
            .into_iter()
            .map(|state| session_look(state).glyph)
            .collect();
        assert_eq!(states.len(), 4);
    }
}
