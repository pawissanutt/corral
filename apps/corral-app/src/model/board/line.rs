//! One task row's facts, resolved to the words and marks a row draws, whether they come from
//! the compiled-in board or from the daemon's.

use crate::model::daemon;
use crate::model::hue::Hue;

use super::{Actor, Mark, Rail, Task, money, rail_label};

/// The cost above which a row draws its cost in the held colour.
const COST_WARNING: f64 = 20.0;

/// Everything a task row shows, owned so a row renders without any store behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLine {
    pub id: &'static str,
    pub title: String,
    /// Who acts next: the row's actor mark and quieter ink for queued and parked tasks.
    pub actor: Actor,
    /// Changed since the last visit.
    pub changed: bool,
    pub rail: Rail,
    /// The now-phrase.
    pub now: String,
    /// The now-phrase's colour: needs you, held, stale (drawn as held), or none.
    pub now_hue: Hue,
    /// `4m`, or empty when unknown.
    pub age: String,
    /// `$3.90`, `~$3.90` for an estimate, or empty without a cost.
    pub cost: String,
    /// Above the $20 warning.
    pub cost_high: bool,
}

/// The colour of a now-phrase for a task `actor` acts on next.
pub fn now_hue(actor: Actor, now: &str) -> Hue {
    match actor {
        Actor::You => Hue::You,
        Actor::Held => Hue::Held,
        _ if now.contains("stale") => Hue::Held,
        _ => Hue::Neutral,
    }
}

/// Who acts next on a daemon task; tasks the daemon cannot place read as queued.
pub const fn daemon_actor(group: daemon::Group) -> Actor {
    match group {
        daemon::Group::NeedsYou => Actor::You,
        daemon::Group::Verifying => Actor::Corral,
        daemon::Group::Producing => Actor::Worker,
        daemon::Group::Held => Actor::Held,
        daemon::Group::Parked => Actor::Parked,
        daemon::Group::Queued | daemon::Group::Other => Actor::Queued,
    }
}

/// `$3.90`, `~$3.90` when the cost is not wholly provider-reported, or nothing without a cost.
fn daemon_cost(cost: Option<f64>, estimated: bool) -> String {
    let amount = money(cost.map(|cost| cost as f32));
    if estimated && !amount.is_empty() {
        format!("~{amount}")
    } else {
        amount
    }
}

impl TaskLine {
    /// The row of a task on the compiled-in board.
    pub fn fixture(task: &Task) -> TaskLine {
        TaskLine {
            id: task.id,
            title: task.title.to_owned(),
            actor: task.actor,
            changed: task.changed,
            rail: task.rail,
            now: task.now.to_owned(),
            now_hue: task.now_hue(),
            age: task.age.unwrap_or_default().to_owned(),
            cost: money(task.cost),
            cost_high: task.hot(),
        }
    }

    /// The row of a daemon task at the daemon's `now` (Unix seconds). The daemon reports no
    /// last-event fact yet, so a daemon row never carries the changed mark.
    pub fn daemon(task: &daemon::Task, now: u64) -> TaskLine {
        let actor = daemon_actor(task.group);
        TaskLine {
            id: task.id,
            title: task.title.clone(),
            actor,
            changed: false,
            rail: std::array::from_fn(|index| task.rail[index].mark),
            now: task.now.clone(),
            now_hue: now_hue(actor, &task.now),
            age: daemon::age(task.since, now),
            cost: daemon_cost(task.cost, task.cost_estimated),
            cost_high: task.cost.is_some_and(|cost| cost > COST_WARNING),
        }
    }

    /// Queued and parked tasks read a step quieter.
    pub fn quiet(&self) -> bool {
        matches!(self.actor, Actor::Queued | Actor::Parked)
    }

    /// The phone's second-line `age · cost`, either alone, or nothing.
    pub fn meta(&self) -> String {
        [self.age.as_str(), self.cost.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// What a screen reader hears: id, title, the rail, the now-phrase, age, cost, and whether
    /// it changed since the last visit; missing parts are left out rather than left empty.
    pub fn label(&self) -> String {
        let mut parts = vec![format!("{} {}", self.id, self.title), rail_label(self.rail)];
        parts.extend(
            [&self.now, &self.age, &self.cost]
                .into_iter()
                .filter(|part| !part.is_empty())
                .cloned(),
        );
        if self.changed {
            parts.push("changed since your last visit".to_owned());
        }
        parts.join(", ")
    }
}

/// A rail stage's state word, as the inspector's stepper shows it.
pub const fn stage_word(mark: Mark) -> &'static str {
    match mark {
        Mark::Done => "done",
        Mark::Active => "now",
        Mark::Fail => "failed",
        Mark::Stale => "stale",
        Mark::Next => "next",
        Mark::Na => "n/a",
    }
}

#[cfg(test)]
mod tests {
    use super::{Task, TaskLine};

    #[test]
    fn row_label_leaves_out_a_missing_age_and_cost_instead_of_empty_parts() {
        let label = TaskLine::fixture(&Task::unknown("T-1")).label();
        assert!(!label.contains(", ,"));
        assert!(!label.ends_with(", "));
    }
}
