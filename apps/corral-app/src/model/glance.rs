//! Facts read at a glance: a count as an icon and a number, the loop band, a task's rail with
//! its current stage named, and a session's recent activity. Each carries the sentence it
//! stands for as a hint; the renderer adds no wording.

use crate::model::board::{Actor, Mark, Rail, STAGES};
use crate::model::daemon;
use crate::model::hue::Hue;
use crate::model::nav::Destination;

/// What a [`Stat`]'s icon says before its number is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatIcon {
    /// Waits on the owner.
    NeedsYou,
    /// Moving: verifying, producing, executing, running.
    Moving,
    /// A result that belongs to an older candidate.
    Stale,
    Landed,
    Failed,
    Queued,
    Parked,
    Held,
    /// Recorded changes.
    Events,
    Sessions,
    Files,
}

impl StatIcon {
    /// The icon in a terminal cell, where no image is drawn.
    pub const fn glyph(self) -> char {
        match self {
            StatIcon::NeedsYou => '●',
            StatIcon::Moving => '◐',
            StatIcon::Stale => '◌',
            StatIcon::Landed => '✓',
            StatIcon::Failed => '✗',
            StatIcon::Queued => '…',
            StatIcon::Parked => '‖',
            StatIcon::Held => '◆',
            StatIcon::Events => '△',
            StatIcon::Sessions => '▣',
            StatIcon::Files => '▤',
        }
    }
}

/// One count: an icon, a number alone, and the sentence the number stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    /// The suffix of the element's `stat-<key>` ID, unique within its group.
    pub key: String,
    pub icon: StatIcon,
    /// `3`, `2h`, `0/3`: never a sentence.
    pub value: String,
    pub hue: Hue,
    /// The complete sentence: `3 things wait on you`.
    pub hint: String,
}

impl Stat {
    pub fn new(
        key: impl Into<String>,
        icon: StatIcon,
        value: impl Into<String>,
        hue: Hue,
        hint: impl Into<String>,
    ) -> Stat {
        Stat {
            key: key.into(),
            icon,
            value: value.into(),
            hue,
            hint: hint.into(),
        }
    }

    /// What a screen reader hears: the hint, which already holds the number.
    pub fn label(&self) -> &str {
        &self.hint
    }
}

/// One stage of the loop a task moves through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopStage {
    /// `plan`, `run`, `gate`, `review` or `land`: the element's `loop-<key>` ID.
    pub key: &'static str,
    pub label: &'static str,
    /// What the stage holds and where pressing it goes.
    pub hint: String,
    /// The stage's counts; empty draws a dash.
    pub stats: Vec<Stat>,
    /// The destination that owns the stage, opened in the band's project scope.
    pub to: Option<Destination>,
}

impl LoopStage {
    /// What a screen reader hears: the stage, then each count's sentence, or that it is empty.
    pub fn spoken(&self) -> String {
        if self.stats.is_empty() {
            return format!("{}, nothing", self.label);
        }
        std::iter::once(self.label)
            .chain(self.stats.iter().map(Stat::label))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Plan, run, gate, review, land, with what each holds now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoopBand {
    pub stages: Vec<LoopStage>,
}

impl LoopBand {
    /// The band's name as a landmark.
    pub const LABEL: &'static str = "Where work sits in the loop";
}

/// A task's rail with the stage it is in named beside its mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageRail {
    pub marks: Rail,
    /// The index of the named stage; none once every stage has passed.
    pub current: Option<usize>,
    /// `gate`, `gate stale`, `run · you`; empty without a current stage.
    pub label: String,
    pub hue: Hue,
    /// The task's now-phrase: what the named stage is doing.
    pub hint: String,
}

impl StageRail {
    /// The rail of a daemon task: the first stage that has not passed is the current one.
    pub fn daemon(task: &daemon::Task, actor: Actor) -> StageRail {
        let marks: Rail = std::array::from_fn(|index| task.rail[index].mark);
        StageRail::of(marks, actor, &task.now)
    }

    /// The rail of `marks` for a task `actor` acts on next, with its now-phrase as the hint.
    pub fn of(marks: Rail, actor: Actor, now: &str) -> StageRail {
        let current = marks
            .iter()
            .position(|mark| !matches!(mark, Mark::Done | Mark::Na));
        let (label, hue) = match current {
            None => (String::new(), Hue::Neutral),
            Some(index) => {
                let stage = STAGES[index];
                match (marks[index], actor) {
                    (_, Actor::You) => (format!("{stage} · you"), Hue::You),
                    (Mark::Stale, _) => (format!("{stage} stale"), Hue::Held),
                    (Mark::Fail, _) => (format!("{stage} failed"), Hue::Fail),
                    (_, Actor::Held) => (format!("{stage} held"), Hue::Held),
                    (Mark::Active, Actor::Worker) => (stage.to_owned(), Hue::Worker),
                    (Mark::Active, _) => (stage.to_owned(), Hue::Corral),
                    _ => (stage.to_owned(), Hue::Neutral),
                }
            }
        };
        StageRail {
            marks,
            current,
            label,
            hue,
            hint: now.to_owned(),
        }
    }
}

/// A session's recorded events per minute, oldest first, ending now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub per_minute: Vec<u16>,
    /// The colour of the newest bar: the session's state.
    pub hue: Hue,
    /// `Last output 38s ago · 14 events in 20 min`.
    pub hint: String,
}

impl Activity {
    /// Each bar's height as a share of the tallest, `0.0..=1.0`; a quiet strip is all zero.
    pub fn heights(&self) -> Vec<f32> {
        let peak = self.per_minute.iter().copied().max().unwrap_or(0);
        self.per_minute
            .iter()
            .map(|&count| {
                if peak == 0 {
                    0.0
                } else {
                    f32::from(count) / f32::from(peak)
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{Actor, Hue, Mark, StageRail};

    #[test]
    fn the_first_unpassed_stage_is_named_and_a_stale_gate_reads_held() {
        let rail = StageRail::of(
            [Mark::Done, Mark::Stale, Mark::Next, Mark::Next],
            Actor::Corral,
            "candidate went stale",
        );
        assert_eq!(rail.current, Some(1));
        assert_eq!(rail.label, "gate stale");
        assert_eq!(rail.hue, Hue::Held);
    }

    #[test]
    fn a_landed_rail_names_no_stage() {
        let rail = StageRail::of([Mark::Done; 4], Actor::Done, "landed");
        assert_eq!(rail.current, None);
        assert!(rail.label.is_empty());
    }
}
