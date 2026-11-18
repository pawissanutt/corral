//! The Plans list: its counts, then each section's plans as two terse lines whose sentences
//! live in hints. A row reads only the plan row (the parent and its per-status child counts)
//! and the board, so the list never waits on a plan's detail.
use corral_protocol::rows::PlanRow;
use corral_types::Status;

use super::live::{
    by_id, current_plan_filtered, matches_project, plan_filter, row_project, topic_loading,
};
use super::{
    Action, ListNotice, LiveAction, MemberShare, OutlineRow, OutlineSecond, OutlineSection,
    PlanOutline, StateIndicator,
};
use crate::model::board::Mark;
use crate::model::daemon::{Feed, Group, Link};
use crate::model::glance::{Stat, StatIcon};
use crate::model::hue::Hue;
use crate::model::projects::ProjectFilter;
use crate::model::{Id, Selection};

/// The destination's title.
pub const TITLE: &str = "Plans";
/// The most members a row draws one segment each; a longer plan draws each state's share.
const STRIP_EACH: usize = 12;

/// Where a plan stands, in the list's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Section {
    NeedsYou,
    Moving,
    NotStarted,
    Done,
    Dropped,
}

const SECTIONS: [(Section, &str, Hue); 5] = [
    (Section::NeedsYou, "Needs you", Hue::You),
    (Section::Moving, "In progress", Hue::Corral),
    (Section::NotStarted, "Not started", Hue::Neutral),
    (Section::Done, "Done", Hue::Ok),
    (Section::Dropped, "Dropped", Hue::Neutral),
];

fn count(row: &PlanRow, status: Status) -> usize {
    row.child_counts
        .iter()
        .filter(|each| each.status == status)
        .map(|each| usize::try_from(each.count).unwrap_or(usize::MAX))
        .sum()
}

/// Members that count toward the plan: every child but the dropped ones.
fn total(row: &PlanRow) -> usize {
    row.child_counts
        .iter()
        .filter(|each| each.status != Status::Dropped)
        .map(|each| usize::try_from(each.count).unwrap_or(usize::MAX))
        .sum()
}

/// The parent or one of its open children waits on the owner.
pub(super) fn needs_you(feed: &Feed, row: &PlanRow) -> bool {
    let parent = row.parent.id.as_str();
    count(row, Status::Blocked) > 0
        || feed
            .board
            .task(parent)
            .is_some_and(|task| task.group == Group::NeedsYou)
        || feed
            .board
            .children(parent)
            .any(|task| task.group == Group::NeedsYou)
}

fn section(feed: &Feed, row: &PlanRow) -> Section {
    match row.parent.status {
        Status::Dropped => Section::Dropped,
        Status::Done => Section::Done,
        _ if needs_you(feed, row) => Section::NeedsYou,
        _ if [Status::Running, Status::Review, Status::Done]
            .into_iter()
            .any(|status| count(row, status) > 0) =>
        {
            Section::Moving
        }
        _ => Section::NotStarted,
    }
}

/// Served plans in the Plans scope by list section, in display order, empty sections left out.
fn grouped<'a>(
    feed: &'a Feed,
    filter: &ProjectFilter,
) -> Vec<(&'static (Section, &'static str, Hue), Vec<&'a PlanRow>)> {
    let filter = plan_filter(feed, filter);
    let mut rows: Vec<(Section, &PlanRow)> = feed
        .plans
        .values()
        .filter(|row| matches_project(row, &filter))
        .map(|row| (section(feed, row), row))
        .collect();
    rows.sort_by(|(a, x), (b, y)| a.cmp(b).then_with(|| by_id(&x.parent.id, &y.parent.id)));
    SECTIONS
        .iter()
        .map(|entry| {
            let rows: Vec<&PlanRow> = rows
                .iter()
                .filter(|(section, _)| *section == entry.0)
                .map(|(_, row)| *row)
                .collect();
            (entry, rows)
        })
        .filter(|(_, rows)| !rows.is_empty())
        .collect()
}

/// What the list says instead of, or above, its rows.
fn list_notice(feed: &Feed, filter: &ProjectFilter, empty: bool) -> Option<ListNotice> {
    let topic = feed.topics.get("plans");
    let error = topic.and_then(|topic| topic.error.as_ref());
    if empty {
        let all = plan_filter(feed, filter) == ProjectFilter::All;
        return Some(if let Some(error) = error {
            ListNotice::Error(format!("corrald could not send plans: {}", error.message))
        } else if feed.link == Link::Down {
            ListNotice::Error("No plans received from corrald yet; the connection is down.".into())
        } else if topic_loading(feed) && feed.plans.is_empty() {
            ListNotice::Loading
        } else {
            ListNotice::Empty {
                heading: if all { "No plans" } else { "No matching plans" },
                body: if all {
                    "A task with children is a plan. corrald serves none yet."
                } else {
                    "No served plan matches this project filter."
                },
            }
        });
    }
    if let Some(error) = error {
        Some(ListNotice::Stale(format!(
            "Showing the last plans received · {}",
            error.message
        )))
    } else if feed.link == Link::Down || topic.is_some_and(|topic| topic.stale) {
        Some(ListNotice::Stale(
            "Showing the last plans received; corrald is not current.".into(),
        ))
    } else {
        None
    }
}

/// Served plan ids in the order the list draws them, for j/k and the first selection.
pub(super) fn display_order(feed: &Feed, filter: &ProjectFilter) -> Vec<Id> {
    grouped(feed, filter)
        .into_iter()
        .flat_map(|(_, rows)| rows)
        .map(|row| Id::new(&row.parent.id))
        .collect()
}

/// The list's counts: plans that need the owner, plans in progress.
fn page_stats(feed: &Feed, filter: &ProjectFilter) -> Vec<Stat> {
    let mut needs = 0;
    let mut moving = 0;
    for ((kind, ..), rows) in grouped(feed, filter) {
        match kind {
            Section::NeedsYou => needs += rows.len(),
            Section::Moving => moving += rows.len(),
            _ => {}
        }
    }
    let mut stats = Vec::new();
    if needs > 0 {
        stats.push(Stat::new(
            "needs-you",
            StatIcon::NeedsYou,
            needs.to_string(),
            Hue::You,
            format!("{needs} plans have a member, or the plan's own task, waiting on you."),
        ));
    }
    if moving > 0 {
        stats.push(Stat::new(
            "moving",
            StatIcon::Moving,
            moving.to_string(),
            Hue::Corral,
            format!("{moving} plans have members under way in this project scope."),
        ));
    }
    stats
}

/// Every plan in the Plans scope by section, the selected one (the root selection, else the
/// first) marked.
pub fn plan_outline(selection: &Selection, feed: &Feed, filter: &ProjectFilter) -> PlanOutline {
    let grouped = grouped(feed, filter);
    let selected = current_plan_filtered(selection, feed, filter);
    let notice = list_notice(feed, filter, grouped.is_empty());
    let sections = grouped
        .into_iter()
        .map(|((_, label, hue), rows)| OutlineSection {
            label: *label,
            hue: *hue,
            count: rows.len(),
            rows: rows
                .into_iter()
                .map(|row| outline_row(feed, row, filter, selected.as_ref()))
                .collect(),
        })
        .collect();
    PlanOutline {
        title: TITLE,
        stats: page_stats(feed, filter),
        notice,
        sections,
    }
}

/// Each status's mark and hue in the member bar, in the bar's order; dropped members are left
/// out.
const BAR: [(Status, Mark, Hue); 7] = [
    (Status::Done, Mark::Done, Hue::Ok),
    (Status::Blocked, Mark::Active, Hue::You),
    (Status::Review, Mark::Active, Hue::Corral),
    (Status::Running, Mark::Active, Hue::Worker),
    (Status::Ready, Mark::Next, Hue::Neutral),
    (Status::Backlog, Mark::Next, Hue::Neutral),
    (Status::Idea, Mark::Next, Hue::Neutral),
];

/// Each state's share of a long plan's members: done, needs you, moving, waiting.
fn shares(segments: &[(Mark, Hue)]) -> Option<Vec<MemberShare>> {
    if segments.len() <= STRIP_EACH {
        return None;
    }
    let share = |hue: Hue, of: &dyn Fn(Hue) -> bool| MemberShare {
        hue,
        count: segments.iter().filter(|(_, each)| of(*each)).count(),
    };
    Some(
        [
            share(Hue::Ok, &|hue| hue == Hue::Ok),
            share(Hue::You, &|hue| hue == Hue::You),
            share(Hue::Corral, &|hue| matches!(hue, Hue::Corral | Hue::Worker)),
            share(Hue::Neutral, &|hue| hue == Hue::Neutral),
        ]
        .into_iter()
        .filter(|share| share.count > 0)
        .collect(),
    )
}

fn state_indicator(feed: &Feed, row: &PlanRow) -> StateIndicator {
    let total = total(row);
    let done = count(row, Status::Done);
    let (icon, hue, hint) = if row.parent.status == Status::Done {
        (StatIcon::Landed, Hue::Ok, "This plan is done.".into())
    } else if needs_you(feed, row) {
        (
            StatIcon::NeedsYou,
            Hue::You,
            "A member of this plan, or the plan's own task, needs you.".into(),
        )
    } else if total > 0 && done == total {
        (
            StatIcon::Landed,
            Hue::Ok,
            "Every member is done; marking the plan's task done is next.".into(),
        )
    } else if count(row, Status::Running) + count(row, Status::Review) > 0 {
        (
            StatIcon::Moving,
            Hue::Corral,
            "Members of this plan are running or in review.".into(),
        )
    } else {
        (
            StatIcon::Queued,
            Hue::Neutral,
            "No member is running, in review or waiting on you.".into(),
        )
    };
    StateIndicator { icon, hue, hint }
}

fn outline_row(
    feed: &Feed,
    row: &PlanRow,
    filter: &ProjectFilter,
    selected: Option<&Id>,
) -> OutlineRow {
    let id = Id::new(&row.parent.id);
    let title = one_line(&row.parent.title);
    let total = total(row);
    let second = if total == 0 {
        OutlineSecond::Status {
            text: "no members".into(),
            hint: "No open or finished task is filed under this plan yet.".into(),
            hue: Hue::Neutral,
        }
    } else {
        let segments: Vec<(Mark, Hue)> = BAR
            .iter()
            .flat_map(|(status, mark, hue)| std::iter::repeat_n((*mark, *hue), count(row, *status)))
            .collect();
        OutlineSecond::Members {
            shares: shares(&segments),
            segments,
            progress: format!("{}/{total}", count(row, Status::Done)),
            state: state_indicator(feed, row),
        }
    };
    let said = match &second {
        OutlineSecond::Members { state, .. } => &state.hint,
        OutlineSecond::Status { hint, .. } => hint,
    };
    OutlineRow {
        test_id: format!("plan-{id}"),
        hint: format!("{title} · {said}"),
        id_label: id.to_string(),
        project_chip: row_project(feed, row, filter),
        selected: selected == Some(&id),
        action: Action::Live(LiveAction::Plan(id.clone())),
        id,
        title,
        second,
    }
}

pub(super) fn one_line(text: &str) -> String {
    let mut line = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    line
}
