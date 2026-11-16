//! A task opened by id that the complete board does not list, a closed task above all: shown
//! read-only from the task detail corrald serves, never through another task's board row. Board
//! facts the detail does not carry (the now-line, agent, cost) are left out. Also the phone's
//! task page, which shows a board task's story or, off the board, this one.

use corral_protocol::board::{RailGlyph, RailStep};
use corral_protocol::detail::TaskDetail;

use crate::model::board::{Actor, Mark};
use crate::model::daemon::{self, Feed, Link};
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::text::span;
use crate::model::{Id, Selection};

use super::live_task::{
    changed_chip, detail_facts, plan_link, progress, record_buttons, run_text, sent_line,
    session_tab, stage, tail_for, verbs,
};
use super::{
    Button, Fact, Inspector, LivePlan, LiveStory, NO_BOARD, NowTab, Press, State, Tail, Verb,
    detail_note, live_evidence, loaded_task, plan_context, task_ref,
};

/// `id`, while the complete board does not list it.
pub fn off_board_id<'a>(feed: &Feed, id: &'a Id) -> Option<&'a Id> {
    (feed.board.complete && feed.board.task(id).is_none()).then_some(id)
}

/// The task the root selects, while the complete board does not list it.
pub fn off_board<'a>(selected: &'a Selection, feed: &Feed) -> Option<&'a Id> {
    selected.task.as_ref().and_then(|id| off_board_id(feed, id))
}

/// The inspector's `tab` for a task off the board: placeholders while its detail loads, the
/// daemon's reason when it cannot be served, else the detail's story, artifacts or run.
pub fn off_board_inspector(
    state: &State,
    feed: &Feed,
    id: &Id,
    tab: NowTab,
    form: Form,
) -> Inspector {
    let Some(detail) = loaded_task(feed, id) else {
        let failed = feed.link == Link::Down
            || feed
                .detail(&task_ref(id))
                .is_some_and(|cache| cache.error.is_some());
        return match detail_note(feed, id) {
            Some(note) if failed => Inspector::Unavailable(note),
            _ => Inspector::Loading,
        };
    };
    match tab {
        NowTab::Story => Inspector::LiveStory(Box::new(story(state, feed, id, detail, form))),
        NowTab::Evidence => Inspector::LiveEvidence(live_evidence(feed, id)),
        NowTab::Session => Inspector::LiveSession(session_tab(feed, id, None)),
    }
}

/// Who acts next, for the story's heading: no one on a closed task, which is named by its
/// status; any other status the board should list but does not yet.
fn heading(status: &str) -> (&'static str, Hue) {
    match status {
        "done" => ("Done", Hue::Ok),
        "dropped" => ("Dropped", Hue::Neutral),
        _ => ("Not on the board", Hue::Neutral),
    }
}

/// A served rail step as the board's step, so the stepper reads the same either way.
fn step(rail: &RailStep) -> daemon::Step {
    daemon::Step {
        mark: match rail.glyph {
            RailGlyph::Pass => Mark::Done,
            RailGlyph::Active => Mark::Active,
            RailGlyph::Fail => Mark::Fail,
            RailGlyph::Stale => Mark::Stale,
            RailGlyph::Pending => Mark::Next,
            RailGlyph::Unavailable | RailGlyph::Other => Mark::Na,
        },
        sha7: rail.sha7.clone(),
        at: rail
            .at
            .as_deref()
            .and_then(|at| at.parse::<jiff::Timestamp>().ok())
            .and_then(|at| u64::try_from(at.as_second()).ok()),
    }
}

/// Session (when its run recorded one) and Ask about this. Ask carries no board revision: the
/// detail is what the owner sees.
pub fn off_board_buttons(feed: &Feed, id: &Id, phone: bool) -> Vec<Button> {
    record_buttons(
        id,
        super::served::session_for(feed, id, None),
        None,
        false,
        phone,
    )
}

/// The story of a task off the board, from its detail alone. The only command offered is one a
/// closed task takes: Revert, for a landed one.
fn story(state: &State, feed: &Feed, id: &Id, detail: &TaskDetail, form: Form) -> LiveStory {
    let (who, who_hue) = heading(&detail.status);
    let mut facts: Vec<(&'static str, Fact)> = Vec::new();
    if let Some(next) = &detail.owner_next {
        facts.push(("You", Fact::Text(vec![span(next.clone()).hue(Hue::You)])));
    }
    detail_facts(detail, feed, &mut facts);
    let run = run_text(Some(detail), None, feed);
    if !run.is_empty() {
        facts.push(("Run", Fact::Text(run)));
    }
    let mut actions = if form == Form::Phone {
        Vec::new()
    } else {
        off_board_buttons(feed, id, false)
    };
    if detail.status == "done" {
        actions.extend(verbs(state, feed, id).into_iter().filter(|button| {
            matches!(
                button.action,
                Some(Press::Verb {
                    verb: Verb::Revert,
                    ..
                })
            )
        }));
    }
    let (progress, progress_note) = progress(Some(detail));
    LiveStory {
        id: id.to_string(),
        title: detail.title.clone(),
        who,
        who_hue,
        plan: detail
            .plan_context
            .as_ref()
            .map(|position| plan_link(&position.plan)),
        changed: changed_chip(feed, id),
        summary: Vec::new(),
        body: Some(detail.body_excerpt.clone()).filter(|body| !body.trim().is_empty()),
        stages: std::array::from_fn(|index| {
            stage(&step(&detail.stages[index]), index, feed.now, Actor::Done)
        }),
        progress,
        progress_note,
        facts,
        actions,
        note: detail_note(feed, id),
        sent: sent_line(state, feed, id),
    }
}

/// What the phone's page shows under "Live": a run's captured lines, a note that it captured
/// none, or nothing at all for a task that never ran.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskLog {
    /// No run, so no log to speak of.
    Absent,
    /// A run that captured no lines.
    Empty,
    Lines(Tail),
}

impl TaskLog {
    fn of(tail: Option<Tail>) -> Self {
        match tail.filter(|tail| !tail.lines.is_empty()) {
            Some(tail) => Self::Lines(tail),
            None => Self::Empty,
        }
    }
}

/// The phone's page for a task opened by id.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskPage {
    /// What shows instead of the task: why there is no board yet, its detail loading, or why
    /// corrald cannot serve it.
    Waiting(Inspector),
    /// The task's story, its Session/Review buttons and its captured log tail.
    Ready {
        story: Box<Inspector>,
        buttons: Vec<Button>,
        log: TaskLog,
    },
}

/// The phone's task page: the board row's story while the board lists the task, its detail's
/// once the complete board does not.
pub fn task_page(state: &State, feed: &Feed, id: &Id) -> TaskPage {
    if let Some(task) = feed.board.task(id) {
        return TaskPage::Ready {
            story: Box::new(super::task_inspector(
                state,
                feed,
                task,
                NowTab::Story,
                Form::Phone,
            )),
            buttons: super::task_buttons(feed, task, true),
            log: TaskLog::of(super::task_tail(feed, task)),
        };
    }
    if !feed.board.complete {
        return TaskPage::Waiting(Inspector::Quiet(
            feed.device.board_notice().unwrap_or(NO_BOARD),
        ));
    }
    match off_board_inspector(state, feed, id, NowTab::Story, Form::Phone) {
        story @ Inspector::LiveStory(_) => TaskPage::Ready {
            story: Box::new(story),
            buttons: off_board_buttons(feed, id, true),
            // A task closed before it ran (dropped) has no run, so no log either way.
            log: if loaded_task(feed, id).is_some_and(|detail| detail.run.is_some()) {
                TaskLog::of(tail_for(feed, id, None))
            } else {
                TaskLog::Absent
            },
        },
        waiting => TaskPage::Waiting(waiting),
    }
}

/// The plan graph under the phone's task page: the board row's plan, or, off the board, the
/// detail's plan position once it has arrived.
pub fn page_plan(feed: &Feed, id: &Id) -> Option<LivePlan> {
    match feed.board.task(id) {
        Some(task) => Some(super::task_plan_context(feed, task)),
        None => off_board_id(feed, id)
            .filter(|id| loaded_task(feed, id).is_some())
            .map(|id| plan_context(feed, id, None)),
    }
}
