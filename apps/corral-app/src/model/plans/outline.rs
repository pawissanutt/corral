//! The Plans list: its counts, then each section's plans as two terse lines whose sentences
//! live in hints.
use corral_protocol::detail::PlanDetail;
use corral_protocol::rows::{PlanRow as WirePlan, PlanState};

use super::live::{current_plan_filtered, list_sections, plan_detail, row_projects};
use super::member_brief::{needs_you, segment};
use super::{
    Action, ListNotice, LiveAction, MemberShare, OutlineRow, OutlineSecond, OutlineSection,
    PlanOutline, StateIndicator,
};
use crate::model::board::Mark;
use crate::model::daemon::{Feed, Group};
use crate::model::glance::{Stat, StatIcon};
use crate::model::hue::Hue;
use crate::model::inbox::owner_moves;
use crate::model::projects::ProjectFilter;
use crate::model::{Id, Selection};

/// The destination's title.
pub const TITLE: &str = "Plans";
/// The most members a row draws one segment each; a longer plan draws each state's share.
const STRIP_EACH: usize = 12;
/// The heading of an approved design's section while it names no tracker.
const NOT_STARTED: &str = "Approved, not started";

/// The list's sections in the order it draws them: by state, an approved design that names no
/// tracker in a section of its own ahead of the approved ones that do. The one order the rows,
/// the keys and the first selection follow.
fn grouped<'a>(
    feed: &'a Feed,
    filter: &ProjectFilter,
) -> (
    Option<ListNotice>,
    Vec<(&'static str, Hue, Vec<&'a WirePlan>)>,
) {
    let (notice, sections) = list_sections(feed, filter);
    let mut drawn = Vec::with_capacity(sections.len() + 1);
    for (label, hue, rows) in sections {
        let (unpublished, published): (Vec<&WirePlan>, Vec<&WirePlan>) =
            rows.into_iter().partition(|row| {
                row.state == PlanState::Approved
                    && row.linked.is_none()
                    && plan_detail(feed, &row.id).is_none_or(|detail| detail.linked.is_none())
            });
        if !unpublished.is_empty() {
            drawn.push((NOT_STARTED, hue, unpublished));
        }
        if !published.is_empty() {
            drawn.push((label, hue, published));
        }
    }
    (notice, drawn)
}

/// Served plan ids in the order the list draws them, for j/k and the first selection.
pub(super) fn display_order(feed: &Feed, filter: &ProjectFilter) -> Vec<Id> {
    grouped(feed, filter)
        .1
        .into_iter()
        .flat_map(|(_, _, rows)| rows)
        .map(|row| Id::from(&row.id))
        .collect()
}

/// Counts plans, not attention rows: several owner moves in one plan still count once.
pub fn page_stats(feed: &Feed, filter: &ProjectFilter) -> Vec<Stat> {
    let mut awaiting = 0;
    let mut held = 0;
    let mut executing = 0;
    for id in display_order(feed, filter) {
        let Some(row) = feed.plans.get(id.as_str()) else {
            continue;
        };
        match row.state {
            PlanState::Awaiting => awaiting += 1,
            PlanState::Executing => {
                executing += 1;
                held += usize::from(plan_needs_you(feed, row));
            }
            _ => {}
        }
    }
    let mut stats = Vec::new();
    let count = awaiting + held;
    if count > 0 {
        stats.push(Stat::new(
            "needs-you", StatIcon::NeedsYou, count.to_string(), Hue::You,
            format!("{count} plans need you: {awaiting} await your acceptance; {held} executing plans have a member waiting on you. Only received owner-attention and member facts are counted."),
        ));
    }
    if executing > 0 {
        stats.push(Stat::new(
            "executing",
            StatIcon::Moving,
            executing.to_string(),
            Hue::Corral,
            format!("{executing} plans are executing in this project scope."),
        ));
    }
    stats
}

pub(super) fn plan_needs_you(feed: &Feed, row: &WirePlan) -> bool {
    let detail = plan_detail(feed, &row.id);
    detail.is_some_and(|detail| detail.members.iter().any(|member| needs_you(feed, member)))
        || feed.board.tasks().iter().any(|task| {
            task.plan.as_deref() == Some(row.id.as_str()) && task.group == Group::NeedsYou
        })
        || owner_moves(feed).any(|item| {
            item.task.as_deref().is_some_and(|task| {
                task != row.id
                    && (item.plan.as_deref() == Some(row.id.as_str())
                        || feed
                            .board
                            .task(task)
                            .is_some_and(|task| task.plan.as_deref() == Some(row.id.as_str()))
                        || detail.is_some_and(|detail| {
                            detail.members.iter().any(|member| member.id == task)
                        }))
            })
        })
}

/// Every plan in the Plans scope by section, the selected one (the root selection, else the
/// first) marked.
pub fn plan_outline(selection: &Selection, feed: &Feed, filter: &ProjectFilter) -> PlanOutline {
    let (notice, grouped) = grouped(feed, filter);
    let selected = current_plan_filtered(selection, feed, filter);
    let sections = grouped
        .into_iter()
        .map(|(label, hue, rows)| OutlineSection {
            label,
            hue,
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

/// Each state's share of a long plan's members, in the order landed, needs you, moving, held,
/// waiting; none for a plan short enough to draw a segment per member.
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
            share(Hue::Held, &|hue| matches!(hue, Hue::Held | Hue::Fail)),
            share(Hue::Neutral, &|hue| hue == Hue::Neutral),
        ]
        .into_iter()
        .filter(|share| share.count > 0)
        .collect(),
    )
}

fn outline_row(
    feed: &Feed,
    row: &WirePlan,
    filter: &ProjectFilter,
    selected: Option<&Id>,
) -> OutlineRow {
    let detail = plan_detail(feed, &row.id);
    let id = Id::from(&row.id);
    let title = one_line(&row.title);
    let second = if matches!(row.state, PlanState::Executing | PlanState::Complete) {
        let segments: Vec<(Mark, Hue)> = detail.map_or_else(Vec::new, |detail| {
            detail
                .members
                .iter()
                .map(|member| segment(feed, member))
                .collect()
        });
        OutlineSecond::Members {
            shares: shares(&segments),
            segments,
            progress: format!("{}/{}", row.members_landed, row.members_total),
            state: state_indicator(feed, row, detail),
        }
    } else {
        let (text, hint) = status(row, detail);
        let hue = match row.state {
            PlanState::Awaiting => Hue::You,
            PlanState::PlanFailed => Hue::Fail,
            _ => Hue::Neutral,
        };
        OutlineSecond::Status { text, hint, hue }
    };
    let said = match &second {
        OutlineSecond::Members { state, .. } => &state.hint,
        OutlineSecond::Status { hint, .. } => hint,
    };
    let projects = row_projects(feed, row, filter);
    let project_chip = projects.first().map(|first| match projects.len() {
        1 => first.clone(),
        n => format!("{first} +{}", n - 1),
    });
    OutlineRow {
        test_id: format!("plan-{}", row.id),
        hint: format!("{title} · {said}"),
        id_label: row.id.clone(),
        project_chip,
        project_hint: (projects.len() > 1).then(|| projects.join(", ")),
        selected: selected == Some(&id),
        action: Action::Live(LiveAction::Plan(id.clone())),
        id,
        title,
        second,
    }
}

fn state_indicator(feed: &Feed, row: &WirePlan, detail: Option<&PlanDetail>) -> StateIndicator {
    let (icon, hue, hint) = if row.state == PlanState::Complete {
        (StatIcon::Landed, Hue::Ok, "This plan is complete.".into())
    } else if plan_needs_you(feed, row) {
        (
            StatIcon::NeedsYou,
            Hue::You,
            "A member of this plan needs you.".into(),
        )
    } else if let Some(detail) = detail {
        let mut stale = false;
        let mut held = false;
        let mut moving = None;
        let mut all_landed = !detail.members.is_empty();
        for member in &detail.members {
            let (mark, hue) = segment(feed, member);
            stale |= mark == Mark::Stale;
            held |= hue == Hue::Held;
            if mark == Mark::Active && moving.is_none() {
                moving = Some(hue);
            }
            all_landed &= mark == Mark::Done;
        }
        if stale {
            (
                StatIcon::Stale,
                Hue::Held,
                "A member of this plan has a stale result.".into(),
            )
        } else if held {
            (
                StatIcon::Held,
                Hue::Held,
                "A member of this plan is held.".into(),
            )
        } else if let Some(hue) = moving {
            (
                StatIcon::Moving,
                hue,
                "Members of this plan are moving.".into(),
            )
        } else if all_landed {
            (
                StatIcon::Landed,
                Hue::Ok,
                "Every member has landed; final acceptance is next.".into(),
            )
        } else {
            (
                StatIcon::Queued,
                Hue::Neutral,
                "No received member state is moving or waiting on you.".into(),
            )
        }
    } else {
        (
            StatIcon::Moving,
            Hue::Corral,
            "This plan is executing; member states have not been received.".into(),
        )
    };
    StateIndicator { icon, hue, hint }
}

fn status(row: &WirePlan, detail: Option<&PlanDetail>) -> (String, String) {
    match row.state {
        PlanState::Awaiting => {
            let revision = row.revision.as_deref();
            let review = detail
                .filter(|detail| detail.revision.as_deref() == revision)
                .and_then(|detail| {
                    detail
                        .reviews
                        .iter()
                        .rev()
                        .find(|review| Some(review.revision.as_str()) == revision)
                });
            let short = match review {
                Some(review) if review.verdict.starts_with("approve") => "reviewer approves".into(),
                Some(review) => format!("reviewer · {}", one_line(&review.verdict)),
                None => format!("{} members", row.members_total),
            };
            let text =
                revision.map_or_else(|| short.clone(), |revision| format!("{revision} · {short}"));
            (
                text,
                match review {
                    Some(review) => format!(
                        "{} awaits your acceptance. The reviewer verdict for {} is {}.",
                        row.id, review.revision, review.verdict
                    ),
                    None => format!(
                        "{} awaits your acceptance and lists {} members. No review for this revision has been received.",
                        row.id, row.members_total
                    ),
                },
            )
        }
        PlanState::Approved
            if row.linked.is_none() && detail.is_none_or(|detail| detail.linked.is_none()) =>
        {
            (
                "no tracker published".into(),
                "This plan is approved; no published tracker is named.".into(),
            )
        }
        PlanState::Approved => (
            "approved".into(),
            "This plan is approved and names a published tracker.".into(),
        ),
        PlanState::Drafting => ("drafting".into(), "The plan is being drafted.".into()),
        PlanState::Reviewing => (
            "in plan review".into(),
            "The plan is in independent review.".into(),
        ),
        PlanState::PlanFailed => (
            "planning failed".into(),
            "Planning or its review failed or escalated.".into(),
        ),
        PlanState::Cancelled => ("cancelled".into(), "This plan is cancelled.".into()),
        _ => (
            "state unknown".into(),
            "The daemon sent a plan state this app does not know.".into(),
        ),
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
