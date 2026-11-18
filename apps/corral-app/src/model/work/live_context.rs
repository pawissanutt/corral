//! What a task's context and the changes feed show in the Daemon state: the plan a task belongs
//! to (its parent task; the phone task page's plan section) and each engine event as a feed row,
//! with why changes cannot be shown.

use corral_protocol::app::{CHANGES_PAGE, CHANGES_V1};
use corral_types::{Actor, Event, EventKind};
use serde_json::Value;

use crate::model::board::{ChangeKind, Mark};
use crate::model::daemon::{Feed, Link};
use crate::model::projects::ProjectFilter;
use crate::model::{Id, Live};

use super::{FeedRow, Press, stamp_at};

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

/// The plan hand-off for task `id`: a plan is its parent task.
pub fn plan_context(id: &Id, parent: Option<&str>) -> LivePlan {
    let plan = parent.map(Id::new);
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

/// A string field of an event's detail.
fn field<'a>(event: &'a Event, key: &str) -> Option<&'a str> {
    event.detail.get(key).and_then(Value::as_str)
}

/// Whether an event records a failure: a red gate, a verdict asking for changes, an error.
pub(super) fn failed(event: &Event) -> bool {
    match event.kind {
        EventKind::Error => true,
        EventKind::Gate => field(event, "result") == Some("fail"),
        EventKind::Verdict => event.detail.get("approve").and_then(Value::as_bool) == Some(false),
        _ => false,
    }
}

/// The mark the feed draws for an event: a failure's cross, a message dot for what needs the
/// owner, a pass for a landing or a green check, a start for runs and moves, a note otherwise.
fn event_mark(event: &Event) -> ChangeKind {
    if failed(event) {
        return ChangeKind::Mark(Mark::Fail);
    }
    match event.kind {
        EventKind::Blocked | EventKind::Ready => ChangeKind::Message,
        EventKind::Landed | EventKind::Gate | EventKind::Verdict => ChangeKind::Mark(Mark::Done),
        EventKind::ReviewStarted | EventKind::SentBack => ChangeKind::Mark(Mark::Na),
        EventKind::Created
        | EventKind::Moved
        | EventKind::Dispatched
        | EventKind::Resumed
        | EventKind::Reopened => ChangeKind::Start,
        _ => ChangeKind::Note,
    }
}

/// `owner`, `agent`, `reviewer`, `assistant`, `auto`.
pub(super) const fn actor_word(actor: Actor) -> &'static str {
    match actor {
        Actor::Owner => "owner",
        Actor::Agent => "agent",
        Actor::Reviewer => "reviewer",
        Actor::Assistant => "assistant",
        Actor::Auto => "auto",
    }
}

/// The first line of a free-text field, so a feed row stays one line.
fn line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

/// What an event says, from its kind and detail: `moved backlog → ready`, `gate failed`,
/// `forced dispatch · skipped unfinished dependencies`.
pub fn event_text(event: &Event) -> String {
    let status = |key| field(event, key).map(|status| status.replace('_', " "));
    match event.kind {
        EventKind::Created => match status("status") {
            Some(status) => format!("created as {status}"),
            None => "created".into(),
        },
        EventKind::Moved => match (status("from"), status("to")) {
            (Some(from), Some(to)) => format!("moved {from} → {to}"),
            _ => "moved".into(),
        },
        EventKind::Commented => match field(event, "text") {
            Some(text) => format!("commented: {}", line(text)),
            None => "commented".into(),
        },
        EventKind::Dispatched => match field(event, "profile") {
            Some(profile) => format!("dispatched · {profile}"),
            None => "dispatched".into(),
        },
        EventKind::Blocked => match field(event, "reason") {
            Some(reason) => format!("blocked: {}", line(reason)),
            None => "blocked".into(),
        },
        EventKind::Done => "worker done".into(),
        EventKind::Gate if failed(event) => "gate failed".into(),
        EventKind::Gate => "gate passed".into(),
        EventKind::ReviewStarted => "review started".into(),
        EventKind::Verdict if failed(event) => "verdict: changes".into(),
        EventKind::Verdict => "verdict: approve".into(),
        EventKind::SentBack => match event.detail.get("round").and_then(Value::as_u64) {
            Some(round) => format!("sent back · round {round}"),
            None => "sent back".into(),
        },
        EventKind::Ready => "ready to land".into(),
        EventKind::Landed => {
            let sha: Option<String> = field(event, "sha").map(|sha| sha.chars().take(7).collect());
            match (sha, field(event, "base")) {
                (Some(sha), Some(base)) => format!("landed {sha} on {base}"),
                (Some(sha), None) => format!("landed {sha}"),
                _ => "landed".into(),
            }
        }
        EventKind::Forced => forced_text(event),
        EventKind::Error => match field(event, "reason") {
            Some(reason) => format!("error: {}", line(reason)),
            None => "error".into(),
        },
        EventKind::Artifact => match field(event, "title") {
            Some(title) => format!("artifact: {title}"),
            None => "artifact added".into(),
        },
        kind => kind.as_str().replace('_', " "),
    }
}

/// `forced move · skipped status rule`: the operation and what force skipped, as recorded.
fn forced_text(event: &Event) -> String {
    let operation = field(event, "operation").unwrap_or("operation");
    let skipped: Vec<&str> = event
        .detail
        .get("skipped")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if skipped.is_empty() {
        format!("forced {operation} · skipped nothing")
    } else {
        format!("forced {operation} · skipped {}", skipped.join(", "))
    }
}

/// One engine event as a feed row: its local time, its task (opening it), what it says and who
/// did it.
pub fn change_row_filtered(
    event: &Event,
    feed: &Feed,
    unseen: bool,
    filter: &ProjectFilter,
) -> FeedRow {
    FeedRow {
        at: stamp_at(event.ts, feed),
        project_chip: event
            .task
            .as_deref()
            .and_then(|id| feed.board.task(id))
            .and_then(|task| crate::model::projects::project_chip(feed, filter, task.project())),
        action: event
            .task
            .as_deref()
            .map(|task| Press::Open(Live::Task(Id::new(task)))),
        id: event.task.clone().unwrap_or_default(),
        kind: event_mark(event),
        text: event_text(event),
        by: Some(actor_word(event.actor).to_owned()),
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
