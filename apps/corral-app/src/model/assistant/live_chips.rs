use corral_protocol::assistant::ContextRef;
use corral_protocol::rows::{AssistantLocation, AssistantRow, AssistantState};

use super::{Chip, needs_you, row, wait_label};
use crate::model::daemon::Feed;
use crate::model::hue::Hue;

/// The thread's state chip from its row, its session stream and its open prompts, as served.
pub(super) fn served_chip(feed: &Feed, scope: &str) -> Chip {
    let streaming = feed.live.get(scope).is_some_and(|live| live.open);
    let Some(row) = row(feed, scope) else {
        let loading = feed
            .topics
            .get("assistant")
            .is_none_or(|topic| topic.loading);
        return Chip::new(if loading { "loading" } else { "unknown" }, Hue::Neutral);
    };
    // An ended session is over: a failure its row still records, a stale stream or an open
    // prompt is history, not a current wait.
    if row.state == AssistantState::Ended {
        return Chip::new("ended", Hue::Neutral);
    }
    if streaming {
        return Chip::new("answering", Hue::Corral);
    }
    if needs_you(feed, scope) {
        return Chip::new("waiting on you", Hue::You);
    }
    if let Some(wait) = &row.wait {
        return Chip::new(
            format!("waiting for assistant · {}", wait_label(wait)),
            Hue::Held,
        );
    }
    let (label, hue) = match row.state {
        AssistantState::Cold if super::live_parts::has_history(feed, scope, row) => {
            ("idle · resumes on your message", Hue::Neutral)
        }
        AssistantState::Cold => ("not started · starts on your message", Hue::Neutral),
        AssistantState::Starting => ("starting", Hue::Corral),
        AssistantState::Ready => ("live", Hue::Worker),
        AssistantState::Working => ("working", Hue::Corral),
        AssistantState::Waiting => ("waiting for assistant", Hue::Held),
        AssistantState::Stopping => ("stopping", Hue::Neutral),
        AssistantState::Paused => ("paused", Hue::Held),
        AssistantState::Ended => ("ended", Hue::Neutral),
        AssistantState::Failed => ("host failed", Hue::Fail),
        AssistantState::Other => ("unknown", Hue::Neutral),
    };
    Chip::new(label, hue)
}

/// Where the session's host runs.
pub(super) fn location(row: Option<&AssistantRow>) -> &'static str {
    match row.map(|row| row.live_in) {
        Some(AssistantLocation::Daemon) => "runs: daemon",
        Some(AssistantLocation::Pane) => "runs in a Pi pane",
        Some(AssistantLocation::None) => "no host",
        Some(AssistantLocation::Other) | None => "host unknown",
    }
}

/// The PM target's chip. corrald reports no PM state, so it stays unknown.
pub(super) fn pm_chip(feed: &Feed) -> Chip {
    if crate::model::WorkspaceMode::of(feed) == crate::model::WorkspaceMode::AssistantOnly {
        return Chip::new("off — no projects", Hue::Neutral);
    }
    Chip::new("unknown", Hue::Neutral)
}

/// The context chip's words: its kind and id, with the task's title when the board has it.
pub(super) fn context_label(feed: &Feed, context: &ContextRef) -> String {
    match feed.board.task(&context.id) {
        Some(task) if context.kind == "task" => format!("{} {}", context.id, task.title()),
        _ => context.id.clone(),
    }
}
