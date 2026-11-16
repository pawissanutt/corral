//! An executing plan at a glance: what holds it up, its member graph and its path to done.
use corral_protocol::detail::PlanDetail;
use corral_protocol::rows::PlanState;

use super::live::open_question;
use super::member_brief::{landed, moving, needs_you, segment};
use super::{ExecutingBrief, graph_input, path_to_done};
use crate::model::Id;
use crate::model::board::Mark;
use crate::model::daemon::Feed;
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};

pub fn plan_brief(feed: &Feed, plan: &Id, detail: &PlanDetail) -> ExecutingBrief {
    ExecutingBrief {
        headline: headline(feed, detail),
        graph: graph_input(feed, plan, detail),
        path: path_to_done(feed, plan, detail),
    }
}

/// The lead sentence in its hue, then how many members have landed.
fn headline(feed: &Feed, detail: &PlanDetail) -> Rich {
    let total = detail.members.len();
    let landed_count = detail
        .members
        .iter()
        .filter(|member| landed(member))
        .count();
    let progress = format!("{landed_count} of {total} landed.");
    let you = detail.members.iter().find(|member| needs_you(feed, member));
    let question = detail
        .members
        .iter()
        .find(|member| needs_you(feed, member) && open_question(feed, &member.id).is_some());
    let held = detail
        .members
        .iter()
        .find(|member| segment(feed, member).1 == Hue::Held);
    let moving_count = detail
        .members
        .iter()
        .filter(|member| moving(feed, member))
        .count();
    let (lead, hue, rest) = if let Some(member) = question {
        (
            format!("{} waits on your answer.", member.id),
            Hue::You,
            Some(progress),
        )
    } else if let Some(member) = you {
        (
            format!("{} needs you.", member.id),
            Hue::You,
            Some(progress),
        )
    } else if let Some(member) = held {
        let state = if segment(feed, member).0 == Mark::Stale {
            "has a stale result"
        } else {
            "is held"
        };
        (format!("{} {state}.", member.id), Hue::Held, Some(progress))
    } else if total > 0 && landed_count == total {
        let next = if detail.state == PlanState::Complete {
            "The plan is complete."
        } else {
            "Final acceptance is next."
        };
        (
            format!("All {total} landed."),
            Hue::Ok,
            Some(next.to_owned()),
        )
    } else if moving_count > 0 {
        let phrase = if moving_count == 1 {
            "1 member is moving.".into()
        } else {
            format!("{moving_count} members are moving.")
        };
        (phrase, Hue::Corral, Some(progress))
    } else if total == 0 {
        (
            "No member tasks are listed in this plan's detail.".into(),
            Hue::Neutral,
            None,
        )
    } else {
        ("No member is moving.".into(), Hue::Neutral, Some(progress))
    };
    let mut line = vec![span(lead).bold().hue(hue)];
    line.extend(rest.map(|rest| span(format!(" {rest}"))));
    line
}
