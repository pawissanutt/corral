//! What Now shows besides the list and the inspector in the Daemon state: the stale flags, the
//! held and parked tasks, the lower tab titles, the goals the board serves and the phone tab,
//! all from the daemon's feed.

use corral_protocol::app::CHANGES_V1;

use crate::model::board::Mark;
use crate::model::daemon::{self, Link};
use crate::model::hue::Hue;
use crate::model::nav::{Destination, Form};
use crate::model::text::span;
use crate::model::{Live, Selection};

use super::{
    Action, Button, ButtonKind, GroupKey, HoldRow, LowTab, PhoneNow, Press, State, Status,
    SummaryChip, daemon_list, group_key, stamp,
};

/// Said by the holds panel when the board holds no held or parked task.
pub const NOTHING_HELD: &str = "Nothing is held or parked.";
/// Said by the Goals panel when no task on the board names a goal.
pub const NO_GOALS: &str = "No task on the board names a goal.";

/// Board-topic failure is independent of socket health.
pub(super) fn board_problem(feed: &daemon::Feed) -> Option<String> {
    let topic = feed.topics.get("board")?;
    if let Some(error) = &topic.error {
        return Some(format!("Board unavailable: {}", error.message));
    }
    (topic.stale && !topic.loading).then(|| "The board subscription is out of date.".into())
}

/// Retained rows are last known while either the topic or its transport is not current.
pub fn daemon_status(feed: &daemon::Feed) -> Status {
    let topic = feed.topics.get("board");
    let stale = feed.link != Link::Up
        || topic.is_some_and(|topic| topic.stale || topic.loading || topic.error.is_some());
    let dimmed = stale && feed.board.complete;
    let problem = board_problem(feed);
    let pending = topic.is_some_and(|topic| topic.loading);
    Status {
        dimmed,
        as_of: dimmed.then(|| {
            feed.synced
                .as_ref()
                .filter(|clock| !clock.is_empty())
                .map_or_else(
                    || "last known board".into(),
                    |clock| format!("last known board · as of {clock}"),
                )
        }),
        banner: problem
            .or_else(|| (dimmed && pending).then(|| "Refreshing the board…".into()))
            .map(|message| super::Banner {
                hue: Hue::Held,
                text: vec![
                    span(message),
                    span(if dimmed {
                        " Showing last known rows."
                    } else {
                        ""
                    }),
                ],
                actions: vec![
                    Button::new("Retry board", Press::RetryBoard, ButtonKind::Ghost)
                        .enabled(feed.link == Link::Up && !pending),
                ],
            }),
    }
}

/// The held and parked tasks of the daemon's board, in the hold table's columns: how long ago
/// the task entered its state, the id, the title with its reason dimmed after it. Each opens
/// its task.
pub fn daemon_holds(feed: &daemon::Feed) -> Vec<HoldRow> {
    feed.board
        .tasks()
        .iter()
        .filter(|task| matches!(group_key(task.group), GroupKey::Held | GroupKey::Parked))
        .map(|task| {
            let mut text = vec![span(task.title.clone())];
            if let Some(reason) = &task.held_reason {
                text.push(span(format!(" · {reason}")).dim());
            } else if group_key(task.group) == GroupKey::Parked {
                text.push(span(format!(" · {}", task.now)).dim());
            }
            HoldRow {
                id: task.id.to_string(),
                since: super::when::age(task.since, feed.now),
                title: task.title.clone(),
                mark: Mark::Na,
                text,
                action: Some(Press::Open(Live::Task(task.id.clone()))),
            }
        })
        .collect()
}

/// A lower dock tab's title in the Daemon state: Since carries this device's last visit; the
/// holds tab counts the board's held and parked tasks once a board has arrived.
pub fn daemon_lower_label(tab: LowTab, feed: &daemon::Feed) -> String {
    let board = &feed.board;
    match tab {
        LowTab::Since => feed
            .seen
            .as_ref()
            .and_then(|seen| seen.at.as_deref())
            .map(|at| stamp(at, feed))
            .filter(|at| !at.is_empty())
            .map_or_else(
                || "Since your last visit".to_owned(),
                |at| format!("Since your last visit · {at}"),
            ),
        LowTab::Parked if board.complete => {
            let held = board.count(daemon::Group::Held);
            match board.count(daemon::Group::Parked) {
                0 => format!("Held {held}"),
                parked => format!("Held {held} · parked {parked}"),
            }
        }
        LowTab::Parked => "Held".into(),
    }
}

/// One goal the board's tasks serve, as a Goals panel row narrowing the list to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalRow {
    pub button: Button,
    pub count: usize,
    pub selected: bool,
}

/// The goals the board's tasks name, each with its task count, after an "All goals" row while
/// the list is narrowed; empty when no task names one.
pub fn daemon_goals(state: &State, feed: &daemon::Feed) -> Vec<GoalRow> {
    let mut goals: Vec<(&str, usize)> = Vec::new();
    for goal in feed
        .board
        .tasks()
        .iter()
        .filter_map(|task| task.goal.as_deref())
    {
        match goals.iter_mut().find(|(known, _)| *known == goal) {
            Some((_, count)) => *count += 1,
            None => goals.push((goal, 1)),
        }
    }
    if goals.is_empty() {
        return Vec::new();
    }
    let mut rows = Vec::with_capacity(goals.len() + 1);
    rows.push(GoalRow {
        button: Button::new("All goals", Press::Goal(None), ButtonKind::Quiet),
        count: feed.board.tasks().len(),
        selected: state.goal.is_none(),
    });
    rows.extend(goals.into_iter().map(|(goal, count)| GoalRow {
        button: Button::new(
            goal.to_owned(),
            Press::Goal(Some(goal.to_owned())),
            ButtonKind::Quiet,
        ),
        count,
        selected: state.goal.as_deref() == Some(goal),
    }));
    rows
}

/// The phone's summary chips from the board and the changes feed: what is new since this
/// device's last visit (when corrald serves changes), what needs the owner, what moves and what
/// waits. None before the first board.
fn daemon_chips(feed: &daemon::Feed) -> Vec<SummaryChip> {
    let board = &feed.board;
    if !board.complete || feed.device.board_notice().is_some() {
        return Vec::new();
    }
    let count = |groups: &[daemon::Group]| {
        let total: usize = groups.iter().map(|group| board.count(*group)).sum();
        u32::try_from(total).unwrap_or(u32::MAX)
    };
    let mut chips = Vec::with_capacity(4);
    if feed.capable(CHANGES_V1)
        && let Some(seen) = &feed.seen
    {
        let unseen = super::live_changes::unseen_count(feed);
        let new = u32::try_from(unseen.count).unwrap_or(u32::MAX);
        let qualifier = if unseen.total { "" } else { " loaded" };
        let since = seen
            .at
            .as_deref()
            .map(|at| stamp(at, feed))
            .filter(|at| !at.is_empty());
        chips.push(SummaryChip::new(
            match since {
                Some(at) => format!("▲ {new}{qualifier} since {at}"),
                None => format!("▲ {new}{qualifier} new"),
            },
            Hue::Neutral,
            new,
        ));
    }
    let you = count(&[daemon::Group::NeedsYou]);
    chips.push(SummaryChip::new(
        format!("{you} need you"),
        if you == 0 { Hue::Neutral } else { Hue::You },
        you,
    ));
    let in_progress = count(&[daemon::Group::Verifying, daemon::Group::Producing]);
    chips.push(SummaryChip::new(
        format!("{in_progress} in progress"),
        Hue::Neutral,
        in_progress,
    ));
    let upcoming = count(&[daemon::Group::Queued, daemon::Group::Other]);
    chips.push(SummaryChip::new(
        format!("{upcoming} upcoming"),
        Hue::Neutral,
        upcoming,
    ));
    chips
}

/// The phone Now tab in the Daemon state: the daemon's list under the board's own summary; a
/// press on the summary opens Changes where corrald serves it.
pub fn daemon_phone_now(state: &State, feed: &daemon::Feed, selected: &Selection) -> PhoneNow {
    PhoneNow {
        title: "Now",
        chips: daemon_chips(feed),
        summary_action: feed
            .capable(CHANGES_V1)
            .then_some(Action::Go(Destination::Changes)),
        list: daemon_list(state, feed, selected, Form::Phone),
        status: daemon_status(feed),
    }
}
