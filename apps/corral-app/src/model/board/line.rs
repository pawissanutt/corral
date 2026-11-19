//! Owned task-row facts projected from the shared daemon feed.

use crate::model::daemon;
use crate::model::hue::Hue;

use super::{Actor, Mark, Rail, rail_label};

/// Everything a task row shows, owned so a row renders without any store behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLine {
    pub id: String,
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

/// Who acts next on a daemon task: backlog and ideas wait on no one and read as parked.
pub const fn daemon_actor(group: daemon::Group) -> Actor {
    match group {
        daemon::Group::NeedsYou => Actor::You,
        daemon::Group::Verifying => Actor::Corral,
        daemon::Group::Producing => Actor::Worker,
        daemon::Group::Backlog | daemon::Group::Ideas => Actor::Parked,
        daemon::Group::Queued | daemon::Group::Other => Actor::Queued,
    }
}

impl TaskLine {
    /// The row of a daemon task at the daemon's `now` (Unix seconds). `changed` says whether
    /// the daemon's changes feed holds an event for it past this device's seen cursor.
    pub fn daemon(task: &daemon::Task, now: u64, changed: bool) -> TaskLine {
        let actor = daemon_actor(task.group);
        let phrase = task.now();
        TaskLine {
            id: task.id.to_string(),
            title: task.title().to_owned(),
            actor,
            changed,
            rail: std::array::from_fn(|index| task.rail[index].mark),
            now_hue: now_hue(actor, &phrase),
            now: phrase,
            age: daemon::age(task.since(), now),
        }
    }

    /// Queued and parked tasks read a step quieter.
    pub fn quiet(&self) -> bool {
        matches!(self.actor, Actor::Queued | Actor::Parked)
    }

    /// What a screen reader hears: id, title, the rail, the now-phrase, age, and whether it
    /// changed since the last visit; missing parts are left out rather than left empty.
    pub fn label(&self) -> String {
        let mut parts = vec![format!("{} {}", self.id, self.title), rail_label(self.rail)];
        parts.extend(
            [&self.now, &self.age]
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
    use super::{Actor, Hue, Mark, TaskLine};

    #[test]
    fn row_label_leaves_out_a_missing_age_instead_of_an_empty_part() {
        let label = TaskLine {
            id: "T-1".into(),
            title: "Task".into(),
            actor: Actor::Queued,
            changed: false,
            rail: [Mark::Next; 4],
            now: "queued".into(),
            now_hue: Hue::Neutral,
            age: String::new(),
        }
        .label();
        assert!(!label.contains(", ,"));
        assert!(!label.ends_with(", "));
    }
}
