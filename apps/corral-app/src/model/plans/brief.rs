//! A plan at a glance: what holds it up, its member graph and its path to done.
use corral_protocol::detail::PlanDetail;

use super::member::Member;
use super::{PlanBrief, graph_input, path_to_done};
use crate::model::Id;
use crate::model::board::Mark;
use crate::model::daemon::Feed;
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};

pub fn plan_brief(feed: &Feed, plan: &Id, detail: &PlanDetail) -> PlanBrief {
    PlanBrief {
        headline: headline(feed, plan, detail),
        graph: graph_input(feed, plan, detail),
        path: path_to_done(feed, plan, detail),
    }
}

/// The lead sentence in its hue, then how many members are done.
fn headline(feed: &Feed, plan: &Id, detail: &PlanDetail) -> Rich {
    let members: Vec<Member> = detail
        .children
        .iter()
        .map(|child| Member::of(feed, child))
        .filter(|member| !member.dropped())
        .collect();
    let total = members.len();
    let done = members.iter().filter(|member| member.done()).count();
    let progress = format!("{done} of {total} done.");
    let moving = members.iter().filter(|member| member.moving()).count();
    let (lead, hue, rest) = if let Some(member) = members.iter().find(|member| member.needs_you())
    {
        (
            format!("{} needs you.", member.id()),
            Hue::You,
            Some(progress),
        )
    } else if let Some(member) = members
        .iter()
        .find(|member| member.segment().0 == Mark::Fail)
    {
        (
            format!("{} failed a check.", member.id()),
            Hue::Held,
            Some(progress),
        )
    } else if total > 0 && done == total {
        (
            format!("All {total} done."),
            Hue::Ok,
            Some(format!("Marking {plan} done is next.")),
        )
    } else if moving > 0 {
        let phrase = if moving == 1 {
            "1 member is moving.".into()
        } else {
            format!("{moving} members are moving.")
        };
        (phrase, Hue::Corral, Some(progress))
    } else if total == 0 {
        ("This plan has no members yet.".into(), Hue::Neutral, None)
    } else {
        ("No member is moving.".into(), Hue::Neutral, Some(progress))
    };
    let mut line = vec![span(lead).bold().hue(hue)];
    line.extend(rest.map(|rest| span(format!(" {rest}"))));
    line
}
