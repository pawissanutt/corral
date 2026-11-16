//! A single ordered explanation of what remains, with actions attached to their facts.
use corral_protocol::detail::PlanDetail;
use corral_protocol::rows::PlanState;

use super::live::open_question;
use super::member_brief::{landed, member_brief, segment};
use super::outline::one_line;
use super::{Action, BriefAction, LiveAction, PathItem, PathStep, PathToDone, PathWarning};
use crate::model::board::Mark;
use crate::model::daemon::Feed;
use crate::model::hue::Hue;
use crate::model::work::when;
use crate::model::{AskTarget, Id, Live};

pub fn path_to_done(feed: &Feed, plan: &Id, detail: &PlanDetail) -> PathToDone {
    let row = feed.plans.get(plan.as_str());
    let complete = detail.state == PlanState::Complete;
    let accepted_at = row.and_then(|row| row.accepted_at.as_deref());
    let accepted = accepted_at.is_some()
        || matches!(
            detail.state,
            PlanState::Executing | PlanState::Approved | PlanState::Complete
        );
    let landed_count = detail
        .members
        .iter()
        .filter(|member| landed(member))
        .count();
    let total = detail.members.len();
    let accepted_hint = match accepted_at {
        Some(at) => {
            let time = when(at, feed);
            format!(
                "The design was accepted {}.",
                if time.is_empty() { at } else { &time }
            )
        }
        None if accepted => {
            "The plan state records an accepted design; its acceptance time is unavailable.".into()
        }
        None => "The design has not been recorded as accepted.".into(),
    };
    let mut steps = vec![PathStep {
        mark: if accepted { Mark::Done } else { Mark::Next },
        hue: if accepted { Hue::Ok } else { Hue::You },
        title: "Design accepted".into(),
        hint: accepted_hint,
        items: Vec::new(),
        warning: None,
    }];
    let mut open: Vec<(usize, PathItem)> = detail
        .members
        .iter()
        .filter(|member| !landed(member))
        .map(|member| {
            let card = member_brief(feed, member);
            let (mark, hue) = segment(feed, member);
            let question = open_question(feed, &member.id);
            let short = if question.is_some() {
                "your answer".to_owned()
            } else if card.needs_you {
                "needs you".to_owned()
            } else if card.rail.label.is_empty() {
                one_line(&member.state.replace('_', " "))
            } else {
                card.rail.label.clone()
            };
            let item = PathItem {
                mark,
                hue,
                text: format!("{} · {short}", member.id),
                hint: if question.is_some() {
                    format!("{} waits on your answer. {}", member.id, card.rail.hint)
                } else {
                    format!("{}: {}", member.id, card.rail.hint)
                },
                action: question.map(|question| BriefAction {
                    label: "Answer",
                    hint: format!("Open the question {question} that {} waits on.", member.id),
                    action: Action::Live(LiveAction::Question(question)),
                }),
            };
            (urgency(hue), item)
        })
        .collect();
    // Stable: members of one urgency keep the plan's order.
    open.sort_by_key(|(rank, _)| *rank);
    let items = capped(open);
    let members_done = landed_count == total;
    steps.push(PathStep {
        mark: if members_done { Mark::Done } else { Mark::Next },
        hue: if members_done { Hue::Ok } else { Hue::Neutral },
        title: format!("Members · {landed_count} of {total}"),
        hint: format!("{landed_count} of {total} members have landed."),
        items,
        warning: None,
    });
    if detail.scenarios != Some(0) {
        let (done, hint, warning) = match (detail.scenarios, detail.mapped) {
            (Some(total), Some(mapped)) if mapped >= total => (
                true,
                format!("All {total} acceptance scenarios are mapped to members."),
                None,
            ),
            (Some(total), Some(mapped)) => {
                let missing = total - mapped;
                let hint = format!(
                    "{missing} of {total} acceptance scenarios are not mapped to any member."
                );
                (
                    false,
                    hint.clone(),
                    Some(PathWarning {
                        text: format!("{missing}/{total} unmapped"),
                        hint,
                        action: ask_mapping(plan, row.map(|row| row.rev)),
                    }),
                )
            }
            (Some(total), None) => {
                let hint = format!(
                    "This plan has {total} acceptance scenarios; the mapped count is unavailable."
                );
                (
                    false,
                    hint.clone(),
                    Some(PathWarning {
                        text: "mapping unknown".into(),
                        hint,
                        action: ask_mapping(plan, row.map(|row| row.rev)),
                    }),
                )
            }
            _ => (
                false,
                "Acceptance scenario and mapping totals are unavailable.".into(),
                None,
            ),
        };
        steps.push(PathStep {
            mark: if done { Mark::Done } else { Mark::Next },
            hue: if done { Hue::Ok } else { Hue::Neutral },
            title: "Aggregate scenarios".into(),
            hint,
            items: Vec::new(),
            warning,
        });
    }
    steps.push(PathStep {
        mark: if complete { Mark::Done } else { Mark::Next },
        hue: if complete {
            Hue::Ok
        } else if detail.accept_blockers.is_empty() {
            Hue::You
        } else {
            Hue::Held
        },
        title: "Final acceptance".into(),
        hint: if complete {
            "The plan is complete; final acceptance is recorded.".into()
        } else if !detail.accept_blockers.is_empty() {
            format!(
                "Final acceptance is held: {}",
                detail.accept_blockers.join("; ")
            )
        } else if detail.acceptable {
            "The daemon reports this plan acceptable; final acceptance is next.".into()
        } else {
            "Final acceptance is not recorded; the daemon has supplied no acceptance blocker."
                .into()
        },
        items: if complete {
            Vec::new()
        } else {
            detail
                .accept_blockers
                .iter()
                .map(|reason| PathItem {
                    mark: Mark::Next,
                    hue: Hue::Held,
                    text: "final acceptance held".into(),
                    hint: reason.clone(),
                    action: None,
                })
                .collect()
        },
        warning: None,
    });
    let current = steps.iter().position(|step| step.mark != Mark::Done);
    if let Some(index) = current {
        steps[index].mark = Mark::Active;
        if index == 1 {
            steps[index].hue = if detail
                .members
                .iter()
                .any(|member| super::member_brief::needs_you(feed, member))
            {
                Hue::You
            } else if detail
                .members
                .iter()
                .any(|member| segment(feed, member).1 == Hue::Held)
            {
                Hue::Held
            } else {
                Hue::Corral
            };
        }
    }
    PathToDone {
        title: "Path to done",
        hint: "What still stands between this plan and its completion, in order.",
        landed: landed_count,
        total,
        current,
        steps,
    }
}

/// The most open members the members step lists; the rest close it as one line.
const PATH_ITEMS: usize = 8;
/// What each urgency is called in the closing line's sentence (needs you is never folded).
const URGENCY_WORDS: [&str; 4] = ["need you", "stale or held", "moving", "waiting"];

/// How soon an open member wants attention, from its segment's hue: needs you, then stale,
/// held or failed, then moving, then waiting.
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

fn ask_mapping(plan: &Id, rev: Option<u64>) -> BriefAction {
    BriefAction {
        label: "Ask PM",
        hint: "Ask the PM to map these scenarios to member tasks.".into(),
        action: Action::Live(LiveAction::Ask {
            record: Live::Plan(plan.clone()),
            rev,
            to: Some(AskTarget::Pm),
            text: Some(format!(
                "Map the acceptance scenarios for {plan} to member tasks and explain who owns any remaining scenarios."
            )),
        }),
    }
}
