//! Shared board vocabulary and feed-backed task-line projection.

mod line;

pub use line::{TaskLine, daemon_actor, now_hue, stage_word};

use crate::model::hue::Hue;

/// The state of one stage of a rail, or of a step, verdict or acceptance line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mark {
    Done,
    Active,
    Fail,
    /// The result belongs to an older candidate. Drawn dashed, so it differs by shape.
    Stale,
    Next,
    Na,
}

impl Mark {
    /// What the mark says to a screen reader.
    pub const fn label(self) -> &'static str {
        match self {
            Mark::Done => "passed",
            Mark::Active => "running",
            Mark::Fail => "failed",
            Mark::Stale => "stale (older candidate)",
            Mark::Next => "not yet",
            Mark::Na => "not applicable",
        }
    }

    /// The mark in a terminal cell.
    pub const fn glyph(self) -> char {
        match self {
            Mark::Done => '✓',
            Mark::Active => '◐',
            Mark::Fail => '✗',
            Mark::Stale => '◌',
            Mark::Next => '○',
            Mark::Na => '·',
        }
    }
}

/// The stages of a rail, in order.
pub const STAGES: [&str; 4] = ["run", "gate", "review", "land"];

/// Run, gate, review, land.
pub type Rail = [Mark; 4];

/// `run passed, gate running, review not yet, land not yet`.
pub fn rail_label(rail: Rail) -> String {
    STAGES
        .iter()
        .zip(rail)
        .map(|(stage, mark)| format!("{stage} {}", mark.label()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Who acts next on a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Actor {
    You,
    Corral,
    Worker,
    Queued,
    Held,
    Parked,
    Done,
}

impl Actor {
    pub const fn hue(self) -> Hue {
        match self {
            Actor::You => Hue::You,
            Actor::Corral => Hue::Corral,
            Actor::Worker => Hue::Worker,
            Actor::Held => Hue::Held,
            Actor::Queued | Actor::Parked | Actor::Done => Hue::Neutral,
        }
    }
}

/// What kind of change a feed line reports; the feed draws a mark for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    /// A stage reached this state.
    Mark(Mark),
    /// A worker started.
    Start,
    /// A checkpoint or other note.
    Note,
    /// A message for you.
    Message,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineKind {
    Context,
    Del,
    Add,
}
