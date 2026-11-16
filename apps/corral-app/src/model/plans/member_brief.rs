//! Member facts shared by outline urgency, graph cards and the path to done.
use corral_protocol::detail::PlanMember;

use super::MemberBrief;
use super::live::rail;
use crate::model::Id;
use crate::model::board::{Actor, Mark, daemon_actor};
use crate::model::daemon::{Feed, Group};
use crate::model::glance::StageRail;
use crate::model::hue::Hue;
use crate::model::inbox::owner_moves;

pub(super) fn landed(member: &PlanMember) -> bool {
    rail(member)[3] == Mark::Done || member.state == "done"
}

pub(super) fn needs_you(feed: &Feed, member: &PlanMember) -> bool {
    !landed(member)
        && (feed
            .board
            .task(&member.id)
            .is_some_and(|task| task.group == Group::NeedsYou)
            || owner_moves(feed).any(|row| row.task.as_deref() == Some(member.id.as_str())))
}

pub(super) fn member_brief(feed: &Feed, member: &PlanMember) -> MemberBrief {
    let task = feed.board.task(&member.id);
    let you = needs_you(feed, member);
    let actor = task.map_or_else(
        || if you { Actor::You } else { Actor::Queued },
        |task| daemon_actor(task.group),
    );
    MemberBrief {
        id: Id::from(&member.id),
        title: task.map_or_else(|| member.title.clone(), |task| task.title.clone()),
        rail: StageRail::of(
            rail(member),
            actor,
            task.map_or(member.state.as_str(), |task| task.now.as_str()),
        ),
        needs_you: you,
    }
}

pub(super) fn segment(feed: &Feed, member: &PlanMember) -> (Mark, Hue) {
    if landed(member) {
        return (Mark::Done, Hue::Ok);
    }
    if needs_you(feed, member) {
        return (Mark::Active, Hue::You);
    }
    let marks = rail(member);
    if marks.contains(&Mark::Stale) {
        return (Mark::Stale, Hue::Held);
    }
    if marks.contains(&Mark::Fail) {
        return (Mark::Fail, Hue::Held);
    }
    let group = feed.board.task(&member.id).map(|task| task.group);
    match group {
        Some(Group::Held) => (Mark::Next, Hue::Held),
        Some(Group::Producing) => (Mark::Active, Hue::Worker),
        Some(Group::Verifying) => (Mark::Active, Hue::Corral),
        _ if marks.contains(&Mark::Active) => (Mark::Active, Hue::Corral),
        _ => (Mark::Next, Hue::Neutral),
    }
}

pub(super) fn moving(feed: &Feed, member: &PlanMember) -> bool {
    let (mark, hue) = segment(feed, member);
    mark == Mark::Active && matches!(hue, Hue::Worker | Hue::Corral)
}
