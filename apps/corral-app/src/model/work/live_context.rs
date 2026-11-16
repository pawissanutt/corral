//! What a task's context and the changes feed show in the Daemon state: the plan a task belongs
//! to (the phone task page's plan section) and each change as a feed row, with why changes
//! cannot be shown.

use corral_protocol::app::{CHANGES_PAGE, CHANGES_V1, ChangeLine};

use crate::model::board::ChangeKind;
use crate::model::daemon::{Feed, Link};
use crate::model::projects::ProjectFilter;
use crate::model::{Id, Live};

use super::{FeedRow, Press, loaded_task, stamp};

/// One quiet line in place of a panel's content, owned or fixed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Quiet(&'static str),
    Said(String),
}

impl Notice {
    pub fn text(&self) -> &str {
        match self {
            Notice::Quiet(text) => *text,
            Notice::Said(text) => text.as_str(),
        }
    }
}

/// Said when corrald serves no changes feed.
pub const CHANGES_NOT_SERVED: &str = "corrald does not serve changes.";

/// The plan graph's hand-off in the Daemon state: the plan the selected task belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePlan {
    pub plan: Option<Id>,
    /// The highlighted member.
    pub member: Option<Id>,
    /// `Plan T-885`.
    pub heading: String,
    /// Said instead of a graph.
    pub empty: Option<Notice>,
    /// Open the plan.
    pub action: Option<Press>,
}

/// The plan of a task: the board's, or the detail's plan position.
pub fn task_plan(feed: &Feed, task: &crate::model::daemon::Task) -> Option<Id> {
    plan_of(feed, &task.id, task.plan.as_deref())
}

fn plan_of(feed: &Feed, id: &str, board: Option<&str>) -> Option<Id> {
    board
        .or_else(|| {
            loaded_task(feed, id)?
                .plan_context
                .as_ref()
                .map(|position| position.plan.as_str())
        })
        .map(Id::from)
}

/// The plan graph hand-off for a task, on the dock or the phone's task page.
pub fn task_plan_context(feed: &Feed, task: &crate::model::daemon::Task) -> LivePlan {
    plan_context(feed, &task.id, task.plan.as_deref())
}

/// The plan graph hand-off for a task by id; `board` is its board row's plan, absent for a
/// task off the board.
pub fn plan_context(feed: &Feed, id: &Id, board: Option<&str>) -> LivePlan {
    let plan = plan_of(feed, id, board);
    LivePlan {
        heading: plan
            .as_ref()
            .map_or_else(|| "Plan".to_owned(), |plan| format!("Plan {plan}")),
        empty: plan
            .is_none()
            .then_some(Notice::Quiet("Not part of a plan.")),
        action: plan.clone().map(|plan| Press::Open(Live::Plan(plan))),
        member: Some(id.clone()),
        plan,
    }
}

/// The mark the feed draws for a change of `class`: a message dot for what needs the owner,
/// a neutral stage mark for verdicts, a note for the rest.
pub fn class_kind(class: &str) -> ChangeKind {
    match class {
        "attention" | "question" => ChangeKind::Message,
        "gate" | "review" | "land" | "annotation" => {
            ChangeKind::Mark(crate::model::board::Mark::Na)
        }
        _ => ChangeKind::Note,
    }
}

/// Older daemons may serve bookkeeping rows without a typed owner-feed kind.
pub(super) fn owner_relevant(line: &ChangeLine) -> bool {
    line.kind != corral_protocol::app::ChangeKind::Other
        || matches!(
            line.class.as_str(),
            "work"
                | "gate"
                | "review"
                | "land"
                | "attention"
                | "question"
                | "comment"
                | "annotation"
                | "automation"
                | "policy"
                | "queue"
                | "plan"
        )
}

fn event_kind(line: &ChangeLine) -> ChangeKind {
    use corral_protocol::app::ChangeKind as Wire;
    if line.failed {
        return ChangeKind::Mark(crate::model::board::Mark::Fail);
    }
    match line.kind {
        Wire::Attention | Wire::Question => ChangeKind::Message,
        Wire::Gate | Wire::Review | Wire::Land | Wire::Annotation => {
            ChangeKind::Mark(crate::model::board::Mark::Na)
        }
        Wire::Created | Wire::Moved | Wire::Work | Wire::Queue => ChangeKind::Start,
        _ => class_kind(&line.class),
    }
}

/// One daemon change as a feed row: its local time, its task (opening it), its words without
/// the leading task id the row already shows, and corrald's attribution.
pub fn change_row_filtered(
    line: &ChangeLine,
    feed: &Feed,
    unseen: bool,
    filter: &ProjectFilter,
) -> FeedRow {
    let id = line.task.clone().unwrap_or_default();
    let words = line
        .text
        .strip_prefix(id.as_str())
        .filter(|_| !id.is_empty())
        .and_then(|rest| rest.strip_prefix(" · ").or_else(|| rest.strip_prefix(' ')))
        .unwrap_or(&line.text);
    let text = match (&line.kind, &line.from, &line.to) {
        (corral_protocol::app::ChangeKind::Moved, Some(from), Some(to)) => {
            format!(
                "moved {} → {}",
                from.replace('_', " "),
                to.replace('_', " ")
            )
        }
        _ if words
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') =>
        {
            words.replace('_', " ")
        }
        _ => words.to_owned(),
    };
    let by = match (line.actor.is_empty(), line.via.is_empty()) {
        (true, true) => None,
        (false, true) => Some(line.actor.clone()),
        (true, false) => Some(format!("via {}", line.via)),
        (false, false) => Some(format!("{} via {}", line.actor, line.via)),
    };
    FeedRow {
        at: stamp(&line.at, feed),
        project_chip: line
            .task
            .as_deref()
            .and_then(|id| feed.board.task(id))
            .and_then(|task| {
                crate::model::projects::project_chip(feed, filter, task.project.as_deref())
            }),
        action: line
            .task
            .as_deref()
            .map(|task| Press::Open(Live::Task(Id::from(task)))),
        id,
        kind: event_kind(line),
        text,
        by,
        unseen,
        dim: !unseen,
        illustrative: false,
    }
}

/// Whether a changes page is on its way.
pub fn changes_loading(feed: &Feed) -> bool {
    feed.operations
        .values()
        .any(|operation| operation.method == CHANGES_PAGE && operation.loading)
}

/// Why changes cannot be shown: not served, loading, failed, or none; `None` once rows exist.
pub(super) fn changes_notice(feed: &Feed) -> Option<Notice> {
    if !feed.capable(CHANGES_V1) {
        return Some(Notice::Quiet(CHANGES_NOT_SERVED));
    }
    if !feed.changes.is_empty() {
        return None;
    }
    if changes_loading(feed) {
        return Some(Notice::Quiet("Loading changes…"));
    }
    if let Some(error) = feed
        .operations
        .values()
        .filter(|operation| operation.method == CHANGES_PAGE)
        .find_map(|operation| operation.error.as_ref())
    {
        return Some(Notice::Said(format!(
            "Changes unavailable: {}",
            error.message
        )));
    }
    if feed.link != Link::Up {
        return Some(Notice::Quiet("No changes received from corrald yet."));
    }
    Some(Notice::Quiet("No changes recorded yet."))
}
