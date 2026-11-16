//! corrald's plans in the shared descriptors: the list's sections from the plan rows, the
//! selected plan's brief, document or approved state from its detail, the member graph from the
//! detail and the board, and the phone page. Absent facts stay absent; nothing here falls back
//! to fixtures.

use std::cmp::Ordering;

use corral_protocol::board::RailGlyph;
use corral_protocol::detail::{DetailBody, DetailRef, PlanDetail, PlanMember};
use corral_protocol::rows::{PlanRole, PlanRow as WirePlan, PlanState as WireState};

use super::live_role::{linked, linked_id, linked_label, role_line};
use super::member_brief::member_brief;
use super::{
    Action, Button, Chip, Detail, ExecutingView, GraphInput, GraphMember, Header, Lens, ListNotice,
    LiveAction, NoticeView, PhonePage, State, live_doc, live_role, live_story, plan_brief,
};
use crate::model::board::{Mark, Rail};
use crate::model::daemon::{DetailCache, Feed, Group, Link};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::inbox::{is_open, owner_moves};
use crate::model::nav::Form;
use crate::model::projects::{ProjectFilter, project_chip};
use crate::model::text::span;
use crate::model::work::when;
use crate::model::{Id, Live, Selection};

/// The list's sections, in display order, with their heading hue.
const SECTIONS: [(&str, WireState, Hue); 9] = [
    ("Awaiting your acceptance", WireState::Awaiting, Hue::You),
    ("Planning failed", WireState::PlanFailed, Hue::Fail),
    ("Drafting", WireState::Drafting, Hue::Neutral),
    ("In plan review", WireState::Reviewing, Hue::Corral),
    ("Executing", WireState::Executing, Hue::Corral),
    // An approved design may already have published its tracker, so not "not started".
    ("Approved", WireState::Approved, Hue::Neutral),
    ("Complete", WireState::Complete, Hue::Ok),
    ("Cancelled", WireState::Cancelled, Hue::Neutral),
    ("Other", WireState::Other, Hue::Neutral),
];

fn section_of(state: WireState) -> usize {
    SECTIONS
        .iter()
        .position(|(_, kind, _)| *kind == state)
        .unwrap_or(SECTIONS.len() - 1)
}

/// `T-9` before `T-10`; ids without a number after those with one.
fn by_id(a: &str, b: &str) -> Ordering {
    fn number(id: &str) -> Option<u64> {
        id.rsplit_once('-')?.1.parse().ok()
    }
    match (number(a), number(b)) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.cmp(b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}

fn plan_filter(feed: &Feed, filter: &ProjectFilter) -> ProjectFilter {
    if feed.capable(corral_protocol::PROJECTS_V1) {
        filter.resolved(feed)
    } else {
        ProjectFilter::All
    }
}

/// Served plans in list order: by section, then by id.
fn sorted<'a>(feed: &'a Feed, filter: &ProjectFilter) -> Vec<&'a WirePlan> {
    let filter = plan_filter(feed, filter);
    let mut rows: Vec<&WirePlan> = feed
        .plans
        .values()
        .filter(|row| matches_project(row, &filter))
        .collect();
    rows.sort_by(|a, b| {
        section_of(a.state)
            .cmp(&section_of(b.state))
            .then_with(|| by_id(&a.id, &b.id))
    });
    rows
}

/// The plan the desktop and terminal detail shows: the root selection while corrald still lists
/// it, otherwise the first plan the list draws.
pub fn current_plan_filtered(
    selection: &Selection,
    feed: &Feed,
    filter: &ProjectFilter,
) -> Option<Id> {
    let filter = plan_filter(feed, filter);
    selection
        .plan
        .clone()
        .filter(|id| {
            feed.plans
                .get(id.as_str())
                .is_some_and(|row| matches_project(row, &filter))
        })
        .or_else(|| {
            super::outline::display_order(feed, &filter)
                .into_iter()
                .next()
        })
}

fn matches_project(row: &WirePlan, filter: &ProjectFilter) -> bool {
    match filter {
        ProjectFilter::All => true,
        ProjectFilter::Project(project) => row.projects.iter().any(|name| name == project.as_str()),
        ProjectFilter::Unassigned => row.projects.is_empty(),
    }
}

/// The cached detail of a plan, whatever its state.
pub(super) fn plan_cache<'a>(feed: &'a Feed, id: &str) -> Option<&'a DetailCache> {
    feed.detail(&DetailRef::Plan { id: id.to_owned() })
}

/// The plan's served detail body, if one has arrived.
pub fn plan_detail<'a>(feed: &'a Feed, id: &str) -> Option<&'a PlanDetail> {
    match plan_cache(feed, id)?.body()? {
        DetailBody::Plan(detail) => Some(detail),
        _ => None,
    }
}

fn topic_loading(feed: &Feed) -> bool {
    feed.topics.get("plans").is_none_or(|topic| topic.loading)
}

/// Served plans in the Plans scope by list section, in display order, empty sections left out,
/// with what the list says instead of, or above, them.
pub(super) fn list_sections<'a>(
    feed: &'a Feed,
    filter: &ProjectFilter,
) -> (
    Option<ListNotice>,
    Vec<(&'static str, Hue, Vec<&'a WirePlan>)>,
) {
    let filter = plan_filter(feed, filter);
    let topic = feed.topics.get("plans");
    let rows = sorted(feed, &filter);
    let notice = if rows.is_empty() {
        Some(
            if let Some(error) = topic.and_then(|topic| topic.error.as_ref()) {
                ListNotice::Error(format!("corrald could not send plans: {}", error.message))
            } else if feed.link == Link::Down {
                ListNotice::Error(
                    "No plans received from corrald yet; the connection is down.".into(),
                )
            } else if topic_loading(feed) && feed.plans.is_empty() {
                ListNotice::Loading
            } else {
                ListNotice::Empty {
                    heading: if filter == ProjectFilter::All {
                        "No plans"
                    } else {
                        "No matching plans"
                    },
                    body: if filter == ProjectFilter::All {
                        "corrald has no plan awaiting acceptance, executing or approved."
                    } else {
                        "No served plan matches this project filter."
                    },
                }
            },
        )
    } else if let Some(error) = topic.and_then(|topic| topic.error.as_ref()) {
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
    };
    let sections = SECTIONS
        .iter()
        .map(|(label, kind, hue)| {
            let rows: Vec<&WirePlan> = rows
                .iter()
                .copied()
                .filter(|row| section_of(row.state) == section_of(*kind))
                .collect();
            (*label, *hue, rows)
        })
        .filter(|(_, _, rows)| !rows.is_empty())
        .collect();
    (notice, sections)
}

/// A list row's projects: the plan's projects while the scope spans several of them, or
/// "Unassigned"; none from a daemon that serves no projects.
pub(super) fn row_projects(feed: &Feed, row: &WirePlan, filter: &ProjectFilter) -> Vec<String> {
    if !feed.capable(corral_protocol::PROJECTS_V1) {
        return Vec::new();
    }
    let filter = plan_filter(feed, filter);
    if row.projects.is_empty() {
        return project_chip(feed, &filter, None).into_iter().collect();
    }
    row.projects
        .iter()
        .filter_map(|project| project_chip(feed, &filter, Some(project.as_str())))
        .collect()
}

/// The projects a plan is filed under, as its head names them.
pub(super) fn project_of(row: Option<&WirePlan>) -> Option<String> {
    row.filter(|row| !row.projects.is_empty())
        .map(|row| row.projects.join(", "))
}

/// A served rail as the app's marks.
pub(super) fn rail(member: &PlanMember) -> Rail {
    member.rail.each_ref().map(|step| match step.glyph {
        RailGlyph::Pass => Mark::Done,
        RailGlyph::Active => Mark::Active,
        RailGlyph::Fail => Mark::Fail,
        RailGlyph::Stale => Mark::Stale,
        RailGlyph::Pending => Mark::Next,
        RailGlyph::Unavailable | RailGlyph::Other => Mark::Na,
    })
}

/// The open canonical question a member waits on, by key.
pub(super) fn open_question(feed: &Feed, task: &str) -> Option<Id> {
    owner_moves(feed)
        .find(|row| row.task.as_deref() == Some(task) && row.question_rev.is_some() && is_open(row))
        .map(|row| Id::from(&row.id))
}

/// corrald's layer numbers (counted from 1) as the graph's display layers from 0, by rank of
/// the distinct numbers, so the first served layer is the one that can start now and no layer
/// the plan does not have is drawn empty. None when any member has no layer (a cycle).
fn served_layers(given: &[Option<u32>]) -> Option<Vec<usize>> {
    let given: Vec<u32> = given.iter().copied().collect::<Option<_>>()?;
    let mut distinct = given.clone();
    distinct.sort_unstable();
    distinct.dedup();
    Some(
        given
            .iter()
            .map(|layer| distinct.partition_point(|known| known < layer))
            .collect(),
    )
}

/// Each member's dependency layer: corrald's when it gives every one, otherwise the longest
/// prerequisite chain within the plan from 0 (bounded, so a cycle cannot loop).
fn layers(detail: &PlanDetail) -> Vec<usize> {
    let given: Vec<Option<u32>> = detail.members.iter().map(|member| member.layer).collect();
    if let Some(given) = served_layers(&given) {
        return given;
    }
    let index = |id: &str| detail.members.iter().position(|member| member.id == id);
    let edges: Vec<(usize, usize)> = detail
        .edges
        .iter()
        .filter_map(|edge| Some((index(&edge.from)?, index(&edge.to)?)))
        .collect();
    let mut depth = vec![0; detail.members.len()];
    for _ in 0..detail.members.len() {
        let mut changed = false;
        for &(from, to) in &edges {
            let deeper = (depth[from] + 1).min(detail.members.len());
            if depth[to] < deeper {
                depth[to] = deeper;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    depth
}

/// The plan's members and edges as the graph draws them, each with its named rail, with live
/// presses: a member with an open question opens it; otherwise placed cards select the member
/// and stacked ones open its task.
pub fn graph_input(feed: &Feed, plan: &Id, detail: &PlanDetail) -> GraphInput {
    let layers = layers(detail);
    let members = detail
        .members
        .iter()
        .zip(layers)
        .map(|(member, layer)| {
            let id = Id::from(&member.id);
            let brief = member_brief(feed, member);
            let hue = match feed.board.task(&member.id).map(|task| task.group) {
                Some(Group::NeedsYou) => Hue::You,
                Some(Group::Verifying) => Hue::Corral,
                Some(Group::Producing) => Hue::Worker,
                Some(Group::Held) => Hue::Held,
                _ if brief.needs_you => Hue::You,
                _ => Hue::Neutral,
            };
            let (select, open) = match open_question(feed, &member.id) {
                Some(key) => (
                    Action::Live(LiveAction::Question(key.clone())),
                    Action::Live(LiveAction::Question(key)),
                ),
                None => (
                    Action::Live(LiveAction::Member {
                        plan: plan.clone(),
                        member: id.clone(),
                    }),
                    Action::Live(LiveAction::Task(id.clone())),
                ),
            };
            GraphMember {
                title: brief.title,
                stage: brief.rail,
                hue,
                needs_you: brief.needs_you,
                layer,
                select,
                open,
                id,
            }
        })
        .collect();
    let edges = detail
        .edges
        .iter()
        .map(|edge| (Id::from(&edge.from), Id::from(&edge.to)))
        .collect();
    GraphInput { members, edges }
}

/// The sentence under a header whose detail is the last copy received, if it is.
pub(super) fn stale_line(cache: &DetailCache, feed: &Feed) -> Option<String> {
    let reason = if feed.link == Link::Down {
        "corrald is not connected".to_owned()
    } else if let Some(error) = &cache.error {
        error.message.clone()
    } else if cache.stale {
        "refreshing".to_owned()
    } else {
        return None;
    };
    let at = cache
        .pages
        .first()
        .map(|page| when(&page.as_of, feed))
        .unwrap_or_default();
    Some(if at.is_empty() {
        format!("Showing the last copy received · {reason}")
    } else {
        format!("Showing the copy received at {at} · {reason}")
    })
}

/// The state chip of a served plan.
pub(super) fn state_chip(state: WireState) -> Chip {
    let (label, hue) = match state {
        WireState::Drafting => ("drafting", Hue::Neutral),
        WireState::Reviewing => ("in plan review", Hue::Corral),
        WireState::PlanFailed => ("plan failed", Hue::Fail),
        WireState::Awaiting => ("needs you", Hue::You),
        WireState::Executing => ("executing", Hue::Corral),
        WireState::Approved => ("approved", Hue::Neutral),
        WireState::Complete => ("complete", Hue::Ok),
        WireState::Cancelled => ("cancelled", Hue::Neutral),
        WireState::Other => ("state unknown", Hue::Neutral),
    };
    Chip {
        label: label.into(),
        hue,
    }
}

fn notice(heading: String, body: String, hue: Hue, action: Option<Button>) -> Detail {
    Detail::Notice(NoticeView {
        heading,
        body,
        hue,
        action,
    })
}

/// The detail of one served plan, or what stands in for it.
pub fn live_detail(state: &State, plan: Option<&Id>, feed: &Feed, env: Env) -> Detail {
    let Some(id) = plan else {
        return if feed.plans.is_empty() {
            notice(
                "No plan to show".into(),
                "Plans appear here when corrald serves them.".into(),
                Hue::Neutral,
                None,
            )
        } else {
            notice(
                "No plan selected".into(),
                "Choose a plan from the list.".into(),
                Hue::Neutral,
                None,
            )
        };
    };
    let row = feed.plans.get(id.as_str());
    let cache = plan_cache(feed, id);
    let detail = plan_detail(feed, id);
    let Some(detail) = detail else {
        if let Some(error) = cache.and_then(|cache| cache.error.as_ref().filter(|_| !cache.loading))
        {
            return notice(
                format!("Could not load plan {id}"),
                error.message.clone(),
                Hue::Fail,
                Some(Button::new(
                    "Retry",
                    Action::Live(LiveAction::Retry(id.clone())),
                )),
            );
        }
        if row.is_none() && !topic_loading(feed) && cache.is_none_or(|cache| !cache.loading) {
            return notice(
                format!("Plan {id} is not served"),
                "corrald does not list this plan.".into(),
                Hue::Neutral,
                None,
            );
        }
        return Detail::Loading;
    };
    let stale = cache.and_then(|cache| stale_line(cache, feed));
    match state.live.lens_of(id) {
        Some(Lens::Diff { from, to }) => {
            return Detail::Diff(live_doc::diff(state, feed, id, row, detail, from, to, env));
        }
        Some(Lens::Document) if detail.document.is_some() => {
            return Detail::Document(live_doc::document_view(state, feed, id, row, detail, stale));
        }
        _ => {}
    }
    match detail.state {
        WireState::Awaiting => {
            Detail::Awaiting(live_doc::awaiting(state, feed, id, row, detail, stale, env))
        }
        WireState::Approved => Detail::Approved(live_role::approved(id, row, detail, stale, env)),
        WireState::Drafting | WireState::Reviewing | WireState::PlanFailed => Detail::Planning(
            super::planning::view(id, row, detail, stale, env.form == Form::Phone),
        ),
        WireState::Executing | WireState::Complete | WireState::Cancelled | WireState::Other => {
            Detail::Executing(executing(state, feed, id, row, detail, stale, env))
        }
    }
}

/// The plan's title, from its row; the id when corrald lists no row for it.
pub(super) fn title(id: &Id, row: Option<&WirePlan>) -> String {
    row.map_or_else(|| id.to_string(), |row| row.title.clone())
}

/// Header commands for a published plan: Ask (in the phone's navigation bar instead), the
/// document when served, and the design a tracker came from.
pub(super) fn tracker_actions(
    id: &Id,
    row: Option<&WirePlan>,
    detail: &PlanDetail,
    phone: bool,
) -> Vec<Button> {
    let mut actions = vec![];
    if !phone {
        actions.push(Button::new(
            "Ask about this",
            Action::Live(LiveAction::Ask {
                record: Live::Plan(id.clone()),
                rev: row.map(|row| row.rev),
                to: None,
                text: None,
            }),
        ));
    }
    if detail.document.is_some() {
        actions.push(
            Button::new(
                "Open document",
                Action::Live(LiveAction::Document(id.clone())),
            )
            .quiet(),
        );
    }
    if let (Some(PlanRole::Tracker), Some(design)) =
        (live_role::role(row, detail), linked(row, detail))
    {
        actions.push(
            Button::new(
                format!("Open design {}", linked_label(&design)),
                Action::Live(LiveAction::Plan(linked_id(&design))),
            )
            .quiet(),
        );
    }
    actions
}

fn executing(
    state: &State,
    feed: &Feed,
    id: &Id,
    row: Option<&WirePlan>,
    detail: &PlanDetail,
    stale: Option<String>,
    env: Env,
) -> ExecutingView {
    let phone = env.form == Form::Phone;
    let selected = state
        .live
        .member_of(id)
        .filter(|member| detail.members.iter().any(|m| m.id == member.as_str()))
        .cloned();
    let mut meta = vec![];
    if let Some(at) = row.and_then(|row| row.accepted_at.as_deref()) {
        let at = when(at, feed);
        meta.push(if at.is_empty() {
            "accepted".to_owned()
        } else {
            format!("accepted {at}")
        });
    }
    if let Some(delivery) = row.and_then(|row| row.delivery.as_deref()) {
        meta.push(format!("delivers to {delivery}"));
    }
    if let Some(line) = role_line(live_role::role(row, detail), linked(row, detail).as_deref()) {
        meta.push(line);
    }
    let story = if phone {
        None
    } else {
        selected.as_ref().and_then(|member| {
            detail
                .members
                .iter()
                .find(|m| m.id == member.as_str())
                .map(|m| live_story::story(feed, id, row, detail, m))
        })
    };
    ExecutingView {
        header: Header {
            plan: Some(id.clone()),
            chips: vec![state_chip(detail.state)],
            id: id.to_string(),
            project: project_of(row),
            title: title(id, row),
            meta: if meta.is_empty() {
                vec![]
            } else {
                vec![span(meta.join(" · ")).dim()]
            },
            illustrative: None,
            actions: tracker_actions(id, row, detail, phone),
            stale,
        },
        brief: plan_brief(feed, id, detail),
        members_heading: "Members",
        members_hint: "Laid out by dependency: a member starts once the members it needs have landed.",
        members_empty: "No members yet. They appear once the plan's tracker is published.",
        selected,
        story,
    }
}

/// The pushed phone page of one served plan.
pub fn live_phone_page(state: &State, id: &Id, feed: &Feed, env: Env) -> PhonePage {
    let row = feed.plans.get(id.as_str());
    let detail = live_detail(
        state,
        Some(id),
        feed,
        Env {
            form: Form::Phone,
            ..env
        },
    );
    let published = matches!(detail, Detail::Executing(_) | Detail::Planning(_));
    PhonePage {
        title: match &detail {
            Detail::Awaiting(_) => "Plan review".into(),
            Detail::Diff(_) => "Revision diff".into(),
            Detail::Document(_) => "Plan document".into(),
            _ => id.to_string(),
        },
        right: published.then(|| {
            Button::new(
                "Ask about this",
                Action::Live(LiveAction::Ask {
                    record: Live::Plan(id.clone()),
                    rev: row.map(|row| row.rev),
                    to: None,
                    text: None,
                }),
            )
        }),
        right_label: published.then_some("Ask the assistant about this plan"),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::served_layers;

    #[test]
    fn canonical_one_based_layers_draw_from_layer_zero_without_an_empty_layer() {
        assert_eq!(
            served_layers(&[Some(1), Some(2), Some(2)]),
            Some(vec![0, 1, 1])
        );
        assert_eq!(served_layers(&[Some(1), None]), None);
    }
}
