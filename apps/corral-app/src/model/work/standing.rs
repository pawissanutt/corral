//! Where a daemon task stands, for its story. The board's broad bucket of tasks no one is acting
//! on ("Upcoming") holds ideas and backlog as well as queued work; the row's machine code says
//! which (`queued` for a ready task, `idea`, `backlog`). Only `queued` is called queued. A
//! daemon that still codes every such task `queued` is corrected by the task's canonical status
//! once its detail has arrived: only `ready` is the queue state the owner sees.

use corral_protocol::detail::TaskDetail;

use crate::model::daemon;
use crate::model::hue::Hue;

use super::{GroupKey, group_key, group_label};

/// The board's machine code of a queued task.
const QUEUED_CODE: &str = "queued";
/// The canonical status that means queued for dispatch.
const QUEUED_STATUS: &str = "ready";

/// Who acts next on the task, and what it is doing now, as its story heads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    pub who: &'static str,
    pub hue: Hue,
    /// The now-phrase: the board's, unless it claims a queue the task is not in.
    pub now: String,
}

/// The heading of a task in the broad bucket, by its machine code or canonical status.
fn heading(code: &str) -> &'static str {
    match code {
        QUEUED_CODE | QUEUED_STATUS => "Queued",
        "idea" => "Idea",
        "backlog" => "Backlog",
        "done" => "Done",
        "dropped" => "Dropped",
        _ => group_label(GroupKey::Queued),
    }
}

/// The task's standing: outside the broad bucket, its group's; inside it, named by its machine
/// code, or by its canonical status where the code claims a queue the status denies.
pub fn standing(task: &daemon::Task, detail: Option<&TaskDetail>) -> Standing {
    let key = group_key(task.group);
    if key != GroupKey::Queued {
        return Standing {
            who: group_label(key),
            hue: key.hue(),
            now: task.now.clone(),
        };
    }
    let denied = detail
        .filter(|_| task.now_code == QUEUED_CODE)
        .map(|detail| detail.status.as_str())
        .filter(|status| *status != QUEUED_STATUS);
    match denied {
        Some(status) => Standing {
            who: heading(status),
            hue: key.hue(),
            now: status.replace('_', " "),
        },
        None => Standing {
            who: heading(&task.now_code),
            hue: key.hue(),
            now: task.now.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Id;
    use crate::model::board::Mark;
    use corral_protocol::board::{RailGlyph, RailStep};

    fn task(code: &str) -> daemon::Task {
        daemon::Task {
            id: Id::from("T-8"),
            title: "filed idea".into(),
            project: None,
            plan: None,
            goal: None,
            group: daemon::Group::Queued,
            rail: std::array::from_fn(|_| daemon::Step {
                mark: Mark::Next,
                sha7: None,
                at: None,
            }),
            now: code.into(),
            now_code: code.into(),
            since: None,
            age_since: None,
            held_reason: None,
            questions: 0,
            attention: Vec::new(),
            agent: None,
            cost: None,
            cost_estimated: false,
            rev: 1,
        }
    }

    fn detail(status: &str) -> TaskDetail {
        TaskDetail {
            id: "T-8".into(),
            owner_revision: "r1".into(),
            title: "filed idea".into(),
            status: status.into(),
            body_excerpt: String::new(),
            stages: std::array::from_fn(|_| RailStep {
                glyph: RailGlyph::Pending,
                sha7: None,
                at: None,
            }),
            dependencies: Vec::new(),
            progress: None,
            owner_next: None,
            hold: None,
            park: None,
            park_revision: None,
            artifacts: Vec::new(),
            run: None,
            plan_context: None,
            log_tail: Vec::new(),
        }
    }

    #[test]
    fn only_a_queued_task_is_headed_queued_in_the_upcoming_bucket() {
        let queued = standing(&task(QUEUED_CODE), None).who;
        // Before its detail arrives, an idea is known by its machine code alone.
        assert_ne!(standing(&task("idea"), None).who, queued);
        // A daemon that codes an idea `queued` is corrected by the canonical status.
        let idea = standing(&task(QUEUED_CODE), Some(&detail("idea")));
        assert_ne!(idea.who, queued);
        assert_ne!(idea.now, QUEUED_CODE);
        let ready = standing(&task(QUEUED_CODE), Some(&detail(QUEUED_STATUS)));
        assert_eq!(ready.who, queued);
    }
}
