//! Now's lower docks in the Daemon state: the selected task's captured log tail with the
//! running agents' tiles, the selected task's plan for the graph, and what changed since this
//! device's last visit.

use corral_protocol::app::{AGENTS_V1, CHANGES_PAGE, CHANGES_V1, ChangeLine};
use corral_protocol::rows::{AgentRow, AgentStatus};

use crate::model::board::ChangeKind;
use crate::model::daemon::{Feed, Link};
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::{Id, Live, Selection};

use super::{
    FeedRow, LiveTile, NO_TAIL, Press, State, Tail, TailLine, daemon_selection, detail_note,
    loaded_task, stamp, task_tail,
};

/// The lines a tile shows of an agent's tail.
const TILE_LINES: usize = 4;
/// The most events the Since panel lists.
const SINCE_ROWS: usize = 50;

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

/// The Live dock in the Daemon state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveNow {
    /// The selected task's captured log tail, when corrald holds one.
    pub selected: Option<Tail>,
    /// Why there is no tail: the detail is loading, failed, or captured nothing.
    pub selected_note: Option<Notice>,
    /// Running agents' tiles.
    pub tiles: Vec<LiveTile>,
    /// Why there are no tiles.
    pub tiles_note: Option<&'static str>,
}

/// Said when no agent is running.
pub const NO_AGENTS: &str = "No agent is running.";
/// Said when corrald serves no agent rows.
pub const AGENTS_NOT_SERVED: &str = "corrald does not serve running agents.";
/// Said when corrald serves no changes feed.
pub const CHANGES_NOT_SERVED: &str = "corrald does not serve changes.";

/// The board's notice when there is no board to show yet, as the list says it.
fn board_notice(feed: &Feed) -> Option<Notice> {
    if let Some(line) = feed.device.board_notice() {
        return Some(Notice::Quiet(line));
    }
    (!feed.board.complete).then_some(Notice::Quiet(if feed.link == Link::Down {
        super::NO_BOARD
    } else {
        "Waiting for the board…"
    }))
}

fn chip(status: AgentStatus) -> (&'static str, Hue) {
    match status {
        AgentStatus::Live => ("live", Hue::Corral),
        AgentStatus::Working => ("working", Hue::Worker),
        AgentStatus::Asking => ("needs you", Hue::You),
        AgentStatus::Gating => ("gating", Hue::Corral),
        AgentStatus::Reviewing => ("reviewing", Hue::Corral),
        AgentStatus::Idle => ("idle", Hue::Neutral),
        AgentStatus::Waiting => ("waiting", Hue::Neutral),
        AgentStatus::Ended => ("ended", Hue::Neutral),
        AgentStatus::Unavailable => ("unavailable", Hue::Held),
        AgentStatus::Other => ("unknown", Hue::Neutral),
    }
}

/// A running agent's tile: its state, what it is on, its last lines as corrald projects them.
fn tile(row: &AgentRow, feed: &Feed) -> LiveTile {
    let (chip, chip_hue) = chip(row.status);
    let session = Id::from(row.id.as_str());
    let lines = &row.tail[row.tail.len().saturating_sub(TILE_LINES)..];
    LiveTile {
        id: row.id.clone(),
        role: row.role.clone(),
        doing: row.now.clone(),
        doing_hue: if row.status == AgentStatus::Asking {
            Hue::You
        } else {
            Hue::Neutral
        },
        profile: row.profile.clone().unwrap_or_default(),
        runtime: row.runtime.clone().unwrap_or_default(),
        pane: row.pane.clone().unwrap_or_else(|| "−".into()),
        chip,
        chip_hue,
        title: row.title.clone(),
        since: row
            .since
            .as_deref()
            .map(|at| stamp(at, feed))
            .filter(|at| !at.is_empty())
            .map(|at| format!("since {at}"))
            .unwrap_or_default(),
        tail: Tail {
            id: row.id.clone(),
            pane: row.pane.clone().unwrap_or_else(|| "−".into()),
            runtime: row.runtime.clone().unwrap_or_default(),
            label: "latest lines".into(),
            snapshot: true,
            illustrative: None,
            lines: lines.iter().map(TailLine::new).collect(),
            cursor: false,
            action: None,
        },
        action: Some(Press::Open(Live::Session(session))),
    }
}

/// The Live dock in the Daemon state: the selected task's captured tail, then a tile for each
/// agent that has not ended.
pub fn daemon_live(state: &State, feed: &Feed, selected: &Selection, form: Form) -> LiveNow {
    if let Some(notice) = board_notice(feed) {
        return LiveNow {
            selected: None,
            selected_note: Some(notice),
            tiles: Vec::new(),
            tiles_note: None,
        };
    }
    let task = daemon_selection(state, selected, &feed.board, form);
    let tail = task.and_then(|task| task_tail(feed, task));
    let selected_note = match (task, &tail) {
        (None, _) => Some(Notice::Quiet(super::EMPTY_BOARD)),
        (Some(task), None) => detail_note(feed, &task.id).map(|note| Notice::Said(note.text)),
        (Some(_), Some(tail)) if tail.lines.is_empty() => Some(Notice::Quiet(NO_TAIL)),
        (Some(_), Some(_)) => None,
    };
    let (tiles, tiles_note) = if feed.capable(AGENTS_V1) {
        let tiles: Vec<LiveTile> = feed
            .agents
            .values()
            .filter(|row| !matches!(row.status, AgentStatus::Ended))
            .map(|row| tile(row, feed))
            .collect();
        let note = tiles.is_empty().then_some(NO_AGENTS);
        (tiles, note)
    } else {
        (Vec::new(), Some(AGENTS_NOT_SERVED))
    };
    LiveNow {
        selected: tail.filter(|tail| !tail.lines.is_empty()),
        selected_note,
        tiles,
        tiles_note,
    }
}

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
    task.plan
        .as_deref()
        .or_else(|| {
            loaded_task(feed, &task.id)?
                .plan_context
                .as_ref()
                .map(|position| position.plan.as_str())
        })
        .map(Id::from)
}

/// The plan graph hand-off for a task, on the dock or the phone's task page.
pub fn task_plan_context(feed: &Feed, task: &crate::model::daemon::Task) -> LivePlan {
    let plan = task_plan(feed, task);
    LivePlan {
        heading: plan
            .as_ref()
            .map_or_else(|| "Plan".to_owned(), |plan| format!("Plan {plan}")),
        empty: plan
            .is_none()
            .then_some(Notice::Quiet("Not part of a plan.")),
        action: plan.clone().map(|plan| Press::Open(Live::Plan(plan))),
        member: Some(task.id.clone()),
        plan,
    }
}

/// The plan graph dock in the Daemon state.
pub fn daemon_plan(state: &State, feed: &Feed, selected: &Selection, form: Form) -> LivePlan {
    let quiet = |notice| LivePlan {
        plan: None,
        member: None,
        heading: String::new(),
        empty: Some(notice),
        action: None,
    };
    if let Some(notice) = board_notice(feed) {
        return quiet(notice);
    }
    match daemon_selection(state, selected, &feed.board, form) {
        Some(task) => task_plan_context(feed, task),
        None => quiet(Notice::Quiet(super::EMPTY_BOARD)),
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

/// One daemon change as a feed row: its local time, its task (opening it), its words without
/// the leading task id the row already shows, and corrald's attribution.
pub fn change_row(line: &ChangeLine, feed: &Feed, unseen: bool) -> FeedRow {
    let id = line.task.clone().unwrap_or_default();
    let text = line
        .text
        .strip_prefix(id.as_str())
        .and_then(|rest| rest.strip_prefix(" · "))
        .filter(|_| !id.is_empty())
        .unwrap_or(&line.text)
        .to_owned();
    let by = match (line.actor.is_empty(), line.via.is_empty()) {
        (true, true) => None,
        (false, true) => Some(line.actor.clone()),
        (true, false) => Some(format!("via {}", line.via)),
        (false, false) => Some(format!("{} via {}", line.actor, line.via)),
    };
    FeedRow {
        at: stamp(&line.at, feed),
        action: line
            .task
            .as_deref()
            .map(|task| Press::Open(Live::Task(Id::from(task)))),
        id,
        kind: class_kind(&line.class),
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

/// What the Since panel shows in the Daemon state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Since {
    pub rows: Vec<FeedRow>,
    pub notice: Option<Notice>,
}

/// The changes past this device's seen cursor, newest first, each opening its task.
pub fn daemon_since(feed: &Feed) -> Since {
    if let Some(notice) = changes_notice(feed) {
        return Since {
            rows: Vec::new(),
            notice: Some(notice),
        };
    }
    let through = feed.seen.as_ref().map_or(0, |seen| seen.through);
    let rows: Vec<FeedRow> = feed
        .changes
        .range(through.saturating_add(1)..)
        .rev()
        .take(SINCE_ROWS)
        .map(|(_, line)| change_row(line, feed, true))
        .collect();
    Since {
        notice: rows
            .is_empty()
            .then_some(Notice::Quiet("Nothing new since your last visit.")),
        rows,
    }
}
