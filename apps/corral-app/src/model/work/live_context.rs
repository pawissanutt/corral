//! What each engine event says, in a task's history and in Activity › Feed, the project it is
//! filed under, and why changes cannot be shown.

use corral_protocol::app::{CHANGES_PAGE, CHANGES_V1};
use corral_types::{Actor, Event, EventKind};
use serde_json::Value;

use crate::model::daemon::{Feed, Link};

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

/// `owner`, `agent`, `reviewer`, `assistant`, `auto`, `extension`.
pub(crate) const fn actor_word(actor: Actor) -> &'static str {
    match actor {
        Actor::Owner => "owner",
        Actor::Agent => "agent",
        Actor::Reviewer => "reviewer",
        Actor::Assistant => "assistant",
        Actor::Auto => "auto",
        Actor::Extension => "extension",
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
            (None, Some(to)) => format!("moved to {to}"),
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

/// What an event says beside its kind, from its detail: `backlog → ready`, `failed`,
/// `9d2f1c0 onto main`; empty when the kind says it all.
pub fn event_detail(event: &Event) -> String {
    let status = |key| field(event, key).map(|status| status.replace('_', " "));
    let first = |key| field(event, key).map(|text| line(text).to_owned());
    match event.kind {
        EventKind::Created => status("status").map(|status| format!("as {status}")),
        EventKind::Moved => match (status("from"), status("to")) {
            (Some(from), Some(to)) => Some(format!("{from} → {to}")),
            (None, Some(to)) => Some(format!("to {to}")),
            _ => None,
        },
        EventKind::Commented => first("text"),
        EventKind::Blocked | EventKind::Error => first("reason"),
        EventKind::Dispatched => {
            field(event, "profile").map(|profile| format!("profile {profile}"))
        }
        EventKind::Gate => Some(if failed(event) { "failed" } else { "passed" }.to_owned()),
        EventKind::Verdict => Some(if failed(event) { "changes" } else { "approve" }.to_owned()),
        EventKind::SentBack => event
            .detail
            .get("round")
            .and_then(Value::as_u64)
            .map(|round| format!("round {round}")),
        EventKind::Landed => {
            let sha: Option<String> = field(event, "sha").map(|sha| sha.chars().take(7).collect());
            match (sha, field(event, "base")) {
                (Some(sha), Some(base)) => Some(format!("{sha} onto {base}")),
                (sha, _) => sha,
            }
        }
        EventKind::Forced => Some(forced_detail(event)),
        EventKind::Artifact => first("title"),
        _ => None,
    }
    .unwrap_or_default()
}

/// What a rule the engine names in a `forced` event's `skipped` list means, in the words the
/// refusal sheet uses for what force skips; a rule this app does not know stays as recorded.
fn skipped_rule(operation: &str, rule: &str, event: &Event) -> String {
    match (operation, rule) {
        (_, "dependencies") => "unfinished dependencies".into(),
        (_, "limits.running") => "the running-slot limit".into(),
        (_, "project.running") => "the project's running-slot limit".into(),
        (_, "gate") => "the gate check".into(),
        ("approve", "status rule") => "the Review status rule".into(),
        ("move", "status rule") => {
            let status = |key| field(event, key).map(|status| status.replace('_', " "));
            match (status("from"), status("to")) {
                (Some(from), Some(to)) => format!("the status rule ({from} → {to})"),
                _ => "the status rule".into(),
            }
        }
        (_, rule) => rule.to_owned(),
    }
}

/// `forced dispatch · skipped unfinished dependencies and the running-slot limit`: the operation
/// and every rule force skipped, as recorded.
fn forced_text(event: &Event) -> String {
    format!("forced {}", forced_detail(event))
}

/// `dispatch · skipped unfinished dependencies and the running-slot limit`.
fn forced_detail(event: &Event) -> String {
    let operation = field(event, "operation").unwrap_or("operation");
    let skipped: Vec<String> = event
        .detail
        .get("skipped")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|rule| skipped_rule(operation, rule, event))
                .collect()
        })
        .unwrap_or_default();
    match skipped.as_slice() {
        [] => format!("{operation} · no rule recorded as skipped"),
        [only] => format!("{operation} · skipped {only}"),
        [rest @ .., last] => format!("{operation} · skipped {} and {last}", rest.join(", ")),
    }
}

/// The project an event is filed under: its own, which the daemon names also for a Done or
/// Dropped task, else its task's board row. `None` when neither is known; `Some(None)` is a
/// task without a project.
pub(crate) fn event_project<'a>(event: &'a Event, feed: &'a Feed) -> Option<Option<&'a str>> {
    match event.project.as_deref() {
        Some(project) => Some(Some(project)),
        None => event
            .task
            .as_deref()
            .and_then(|id| feed.board.task(id))
            .map(|task| task.project()),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Imported moves carry only `to`; they read as a move to it, not a bare "moved".
    #[test]
    fn a_move_with_only_a_destination_names_it() {
        let event = Event {
            seq: 1,
            ts: jiff::Timestamp::UNIX_EPOCH,
            task: Some("T-1".into()),
            run: None,
            project: None,
            kind: EventKind::Moved,
            actor: corral_types::Actor::Owner,
            detail: serde_json::json!({"to": "in_progress"}),
        };
        assert!(event_text(&event).ends_with("to in progress"));
    }
}
