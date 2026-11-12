//! The board snapshot every screen reads (2026-11-07 22:12): tasks with their rails, what changed
//! since the last visit, and the patch hunk both the question and the review show.

mod data;
mod line;

pub use line::{TaskLine, daemon_actor, now_hue, stage_word};

use crate::model::hue::Hue;

/// The snapshot's clock and the owner's last visit on this device.
pub const NOW: &str = "22:12";
pub const LAST_VISIT: &str = "18:40";

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

/// One task of the snapshot. Fields the snapshot lacks for a task are `None` or empty.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Task {
    pub id: &'static str,
    pub title: &'static str,
    /// The plan (tracker) the task is a member of.
    pub plan: Option<&'static str>,
    pub actor: Actor,
    pub rail: Rail,
    /// The now-phrase: `needs you · restore policy`, `quick gate 1751bd2`, `after T-890`.
    pub now: &'static str,
    pub age: Option<&'static str>,
    /// Dollars spent so far.
    pub cost: Option<f32>,
    /// Changed since the last visit.
    pub changed: bool,
    pub agent: Option<&'static str>,
    pub run: Option<&'static str>,
    pub started: Option<&'static str>,
    pub machine: Option<&'static str>,
    pub round: Option<u8>,
    pub goal: Option<&'static str>,
    /// What the task waits on, each with its state.
    pub needs: &'static [(&'static str, Mark)],
    pub unblocks: &'static [&'static str],
    pub steps: &'static [(Mark, &'static str)],
    /// Who moves next, and what they do.
    pub next: Option<(&'static str, &'static str)>,
    pub pane: Option<&'static str>,
    /// The last lines of the task's terminal.
    pub tail: &'static [&'static str],
}

impl Task {
    /// A task the snapshot does not hold: its id as the title, an untouched rail, queued.
    pub const fn unknown(id: &'static str) -> Task {
        Task {
            id,
            title: id,
            plan: None,
            actor: Actor::Queued,
            rail: [Mark::Next; 4],
            now: "",
            age: None,
            cost: None,
            changed: false,
            agent: None,
            run: None,
            started: None,
            machine: None,
            round: None,
            goal: None,
            needs: &[],
            unblocks: &[],
            steps: &[],
            next: None,
            pane: None,
            tail: &[],
        }
    }

    /// The colour of the now-phrase: needs you, held, stale (drawn as held), or none.
    pub fn now_hue(&self) -> Hue {
        now_hue(self.actor, self.now)
    }

    /// Above the $20 warning.
    pub fn hot(&self) -> bool {
        self.cost.is_some_and(|cost| cost > 20.0)
    }
}

/// Every task of the snapshot, in the snapshot's order.
pub fn tasks() -> &'static [Task] {
    data::TASKS
}

/// The task, when the snapshot holds it.
pub fn find(id: &str) -> Option<Task> {
    data::TASKS.iter().copied().find(|task| task.id == id)
}

/// The task, or [`Task::unknown`] for an id the snapshot does not hold.
pub fn task(id: &'static str) -> Task {
    find(id).unwrap_or(Task::unknown(id))
}

/// `$3.90`, `$18`, or nothing without a cost.
pub fn money(cost: Option<f32>) -> String {
    match cost {
        None => String::new(),
        Some(cost) if cost >= 10.0 => format!("${cost:.0}"),
        Some(cost) => format!("${cost:.2}"),
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

/// One line of the changes feed, newest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub at: &'static str,
    pub id: &'static str,
    pub kind: ChangeKind,
    pub text: &'static str,
}

pub fn changes() -> &'static [Change] {
    data::CHANGES
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineKind {
    Context,
    Del,
    Add,
}

/// A patch hunk as it is on main.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk {
    pub file: &'static str,
    /// The hunk header: `@@ -1959,13 +1990,16 @@ class AgentSession`.
    pub at: &'static str,
    /// The first old and new line numbers.
    pub start: (u32, u32),
    pub lines: &'static [(LineKind, &'static str)],
}

/// The `clearQueue` hunk of the Pi 1.5.0 patch that question T-868/R1/Q1 is about.
pub fn hunk() -> &'static Hunk {
    &data::HUNK
}

/// The board's totals by who acts next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub you: u32,
    pub corral: u32,
    pub worker: u32,
    pub queued: u32,
    pub held: u32,
    pub parked: u32,
    pub backlog: u32,
    pub ideas: u32,
    pub plans: u32,
}

pub const COUNTS: Counts = Counts {
    you: 3,
    corral: 3,
    worker: 2,
    queued: 23,
    held: 1,
    parked: 32,
    backlog: 107,
    ideas: 55,
    plans: 6,
};
