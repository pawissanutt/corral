//! Path to done: the plan's children not yet Done, most urgent first, each Blocked one with the
//! first next step the engine served for it, then the plan's own task, which the owner marks
//! done once its members are.
use corral_protocol::detail::PlanDetail;
use corral_types::{Blocked, Status};

use super::member::Member;
use super::outline::one_line;
use super::{Action, BriefAction, LiveAction, PathItem, PathStep, PathToDone, PathWarning};
use crate::model::Id;
use crate::model::act;
use crate::model::board::Mark;
use crate::model::daemon::Feed;
use crate::model::hue::Hue;

/// The first of a Blocked task's served next steps, as a press; none when it serves none.
pub(super) fn next_step(task: &Id, blocked: &Blocked) -> Option<BriefAction> {
    let next = blocked.next.first()?;
    Some(BriefAction {
        label: act::label(&next.op, next.force),
        action: Action::Live(LiveAction::Next {
            task: task.clone(),
            index: 0,
        }),
        hint: next.command(),
    })
}

pub fn path_to_done(feed: &Feed, plan: &Id, detail: &PlanDetail) -> PathToDone {
    let members: Vec<Member> = Member::all(feed, detail)
        .into_iter()
        .filter(|member| !member.dropped())
        .collect();
    let total = members.len();
    let done = members.iter().filter(|member| member.done()).count();
    let mut open: Vec<(usize, PathItem)> = members
        .iter()
        .filter(|member| member.open())
        .map(|member| {
            let (mark, hue) = member.segment();
            let rail = member.rail();
            let short = if member.needs_you() {
                "needs you".to_owned()
            } else if rail.label.is_empty() {
                member.status().as_str().to_owned()
            } else {
                rail.label.clone()
            };
            let id = Id::new(member.id());
            let item = PathItem {
                mark,
                hue,
                text: format!("{id} · {short}"),
                hint: format!("{id} {}: {}", one_line(member.title()), member.now()),
                action: member.blocked().and_then(|blocked| next_step(&id, blocked)),
            };
            (urgency(hue), item)
        })
        .collect();
    // Stable: members of one urgency keep the plan's order.
    open.sort_by_key(|(rank, _)| *rank);
    let members_done = done == total;
    let parent = feed
        .board
        .task(plan)
        .map_or(&detail.parent, |row| row.task());
    let complete = parent.status == Status::Done;
    let mut steps = vec![
        PathStep {
            mark: if members_done { Mark::Done } else { Mark::Next },
            hue: if members_done { Hue::Ok } else { Hue::Neutral },
            title: match total - done {
                _ if total == 0 => "Members · none yet".to_owned(),
                0 => format!("Members · all {total} done"),
                left => format!("Members · {left} of {total} not yet done"),
            },
            hint: format!(
                "{} of {total} children are not yet Done; dropped children do not count.",
                total - done
            ),
            items: capped(open),
            warning: None,
        },
        PathStep {
            mark: if complete { Mark::Done } else { Mark::Next },
            hue: if complete { Hue::Ok } else { Hue::You },
            title: format!("{plan} done"),
            hint: if complete {
                "The plan's own task is done.".into()
            } else {
                format!("Mark {plan} done once its members are; its Actions menu has Mark done.")
            },
            items: Vec::new(),
            warning: parent.blocked.as_ref().map(|blocked| PathWarning {
                text: blocked.reason.clone(),
                hint: blocked
                    .detail
                    .clone()
                    .unwrap_or_else(|| format!("{plan} is blocked: {}", blocked.reason)),
                action: next_step(plan, blocked),
            }),
        },
    ];
    let current = steps.iter().position(|step| step.mark != Mark::Done);
    if let Some(index) = current {
        steps[index].mark = Mark::Active;
        if index == 0 {
            steps[index].hue = if members.iter().any(Member::needs_you) {
                Hue::You
            } else {
                Hue::Corral
            };
        }
    }
    PathToDone {
        title: "Path to done",
        hint: "What still stands between this plan and its completion, in order.",
        done,
        total,
        current,
        steps,
    }
}

/// The most open members the members step lists; the rest close it as one line.
const PATH_ITEMS: usize = 8;
/// What each urgency is called in the closing line's sentence (needs you is never folded).
const URGENCY_WORDS: [&str; 4] = ["need you", "failed", "moving", "waiting"];

/// How soon an open member wants attention, from its segment's hue: needs you, then failed,
/// then moving, then waiting.
fn urgency(hue: Hue) -> usize {
    match hue {
        Hue::You => 0,
        Hue::Held | Hue::Fail => 1,
        Hue::Corral | Hue::Worker => 2,
        Hue::Neutral | Hue::Ok => 3,
    }
}

/// The first `PATH_ITEMS` open members by urgency, every one that needs the owner however many,
/// then one line for the rest: `+32 more`, its hint how many of them are in each state.
fn capped(open: Vec<(usize, PathItem)>) -> Vec<PathItem> {
    let needs_you = open.iter().filter(|(rank, _)| *rank == 0).count();
    let shown = PATH_ITEMS.max(needs_you);
    if open.len() <= shown {
        return open.into_iter().map(|(_, item)| item).collect();
    }
    let rest = open.len() - shown;
    let mut counts = [0usize; 4];
    for (rank, _) in &open[shown..] {
        counts[*rank] += 1;
    }
    let states = counts
        .iter()
        .zip(URGENCY_WORDS)
        .filter(|(count, _)| **count > 0)
        .map(|(count, words)| format!("{count} {words}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut items: Vec<PathItem> = open.into_iter().take(shown).map(|(_, item)| item).collect();
    items.push(PathItem {
        mark: Mark::Next,
        hue: Hue::Neutral,
        text: format!("+{rest} more"),
        hint: format!(
            "{rest} more member{}: {states}",
            if rest == 1 { "" } else { "s" }
        ),
        action: None,
    });
    items
}
