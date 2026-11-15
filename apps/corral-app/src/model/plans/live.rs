//! corrald's plans in the shared descriptors: the grouped list from the plan rows, the selected
//! plan's tracker or approved state from its detail, the member graph from the detail and the
//! board, and the phone page. Absent facts stay absent; nothing here falls back to fixtures.

use std::cmp::Ordering;

use corral_protocol::board::RailGlyph;
use corral_protocol::detail::{DetailBody, DetailRef, PlanDetail, PlanMember};
use corral_protocol::rows::{InboxState, PlanRole, PlanRow as WirePlan, PlanState as WireState};

use super::live_role::{linked, linked_id, linked_label, role_line};
use super::view::{pipeline, pipeline_marks, square};
use super::{
    Action, Blocker, Button, Chip, Detail, ExecutingView, GraphInput, GraphMember, Header, Lens,
    ListNotice, ListSection, LiveAction, MemberSquare, NoticeView, PhonePage, PlanList, PlanRow,
    SquareState, State, live_doc, live_role, live_story,
};
use crate::model::board::{Mark, Rail};
use crate::model::daemon::{DetailCache, Feed, Group, Link};
use crate::model::effect::{Effect, Env};
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::text::span;
use crate::model::work::when;
use crate::model::{Id, Live, Selection};

/// The list's sections, in display order, with their heading hue.
const SECTIONS: [(&str, WireState, Hue); 6] = [
    ("Awaiting your acceptance", WireState::Awaiting, Hue::You),
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

/// Served plans in list order: by section, then by id.
fn sorted(feed: &Feed) -> Vec<&WirePlan> {
    let mut rows: Vec<&WirePlan> = feed.plans.values().collect();
    rows.sort_by(|a, b| {
        section_of(a.state)
            .cmp(&section_of(b.state))
            .then_with(|| by_id(&a.id, &b.id))
    });
    rows
}

/// Served plan ids in list order, for j/k.
pub(super) fn order(feed: &Feed) -> Vec<Id> {
    sorted(feed)
        .into_iter()
        .map(|row| Id::from(&row.id))
        .collect()
}

/// The plan the desktop and terminal detail shows: the root selection while corrald still lists
/// it, otherwise the first plan of the list.
pub fn current_plan(selection: &Selection, feed: &Feed) -> Option<Id> {
    selection
        .plan
        .clone()
        .filter(|id| feed.plans.contains_key(id.as_str()))
        .or_else(|| order(feed).into_iter().next())
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

/// Grouped list of corrald's plans with its loading, empty, error and stale states.
pub fn live_list(selection: &Selection, feed: &Feed, env: Env) -> PlanList {
    let phone = env.form == Form::Phone;
    let topic = feed.topics.get("plans");
    let rows = sorted(feed);
    let notice = if rows.is_empty() {
        Some(
            if let Some(error) = topic.and_then(|topic| topic.error.as_ref()) {
                ListNotice::Error(format!("corrald could not send plans: {}", error.message))
            } else if feed.link == Link::Down {
                ListNotice::Error(
                    "No plans received from corrald yet; the connection is down.".into(),
                )
            } else if topic_loading(feed) {
                ListNotice::Loading
            } else {
                ListNotice::Empty {
                    heading: "No plans",
                    body: "corrald has no plan awaiting acceptance, executing or approved.",
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
    let selected = if phone {
        None
    } else {
        current_plan(selection, feed)
    };
    let sections = SECTIONS
        .iter()
        .map(|(label, kind, hue)| ListSection {
            label: *label,
            count: None,
            hue: *hue,
            illustrative: None,
            rows: rows
                .iter()
                .filter(|row| section_of(row.state) == section_of(*kind))
                .map(|row| list_row(feed, row, selected.as_ref(), phone))
                .collect(),
        })
        .filter(|section| !section.rows.is_empty())
        .map(|mut section| {
            section.count = u32::try_from(section.rows.len()).ok();
            section
        })
        .collect();
    PlanList {
        title: "Plans",
        large: phone,
        notice,
        sections,
    }
}

fn list_row(feed: &Feed, row: &WirePlan, selected: Option<&Id>, phone: bool) -> PlanRow {
    let id = Id::from(&row.id);
    let tracking = matches!(row.state, WireState::Executing | WireState::Complete);
    let sub = match row.state {
        WireState::Drafting => vec![span("drafting").dim()],
        WireState::Reviewing => vec![span("in plan review").dim()],
        WireState::PlanFailed => vec![span("plan failed").hue(Hue::Fail)],
        WireState::Awaiting => vec![
            span(match &row.revision {
                Some(rev) => format!("revision {rev} · {} members", row.members_total),
                None => format!("{} members", row.members_total),
            })
            .dim(),
        ],
        // The row lists the members not yet done (corrald sends at most four); the detail's
        // final-acceptance blockers are a different, wider fact and may number otherwise.
        WireState::Executing if row.blockers.is_empty() => {
            vec![span("every member done").dim()]
        }
        WireState::Executing => {
            let n = row.blockers.len();
            let more = n == 4 && row.members_total > 4;
            vec![
                span(format!(
                    "{n}{} unfinished member{}",
                    if more { "+" } else { "" },
                    if n == 1 && !more { "" } else { "s" }
                ))
                .hue(Hue::Held),
            ]
        }
        WireState::Approved if row.linked.is_some() => vec![span("approved").dim()],
        WireState::Approved => vec![span("approved · no tracker published").dim()],
        WireState::Complete => vec![span("complete").dim()],
        WireState::Cancelled => vec![span("cancelled").dim()],
        WireState::Other => vec![span("state not known to this app").dim()],
    };
    // A design and the tracker it published can share a title; the row says which is which.
    let sub = match role_line(row.role, row.linked.as_deref()) {
        Some(role) => {
            let mut line = vec![span(format!("{role} · ")).dim()];
            line.extend(sub);
            line
        }
        None => sub,
    };
    PlanRow {
        id_label: match (&row.revision, phone) {
            (Some(rev), false) => format!("{} {rev}", row.id),
            _ => row.id.clone(),
        },
        rev: row.revision.clone(),
        title: row.title.clone(),
        hue: SECTIONS[section_of(row.state)].2,
        sub,
        members: if tracking {
            plan_detail(feed, &row.id).map_or_else(Vec::new, |detail| squares(feed, detail))
        } else {
            vec![]
        },
        progress: tracking.then(|| format!("{}/{}", row.members_landed, row.members_total)),
        selected: selected == Some(&id),
        illustrative: None,
        action: Action::Live(LiveAction::Plan(id.clone())),
        id,
    }
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
    feed.inbox
        .values()
        .find(|row| {
            row.task.as_deref() == Some(task)
                && row.question_rev.is_some()
                && matches!(row.state, InboxState::Open)
        })
        .map(|row| Id::from(&row.id))
}

/// Who acts next on a member, from the board when it lists the task.
fn member_group(feed: &Feed, member: &PlanMember) -> Option<Group> {
    feed.board.task(&member.id).map(|task| task.group)
}

fn needs_you(feed: &Feed, member: &PlanMember) -> bool {
    member_group(feed, member) == Some(Group::NeedsYou) || open_question(feed, &member.id).is_some()
}

fn squares(feed: &Feed, detail: &PlanDetail) -> Vec<MemberSquare> {
    detail
        .members
        .iter()
        .map(|member| {
            let state = if needs_you(feed, member) {
                SquareState::You
            } else if rail(member)[3] == Mark::Done {
                SquareState::Landed
            } else {
                match member_group(feed, member) {
                    Some(Group::Verifying | Group::Producing) => SquareState::Active,
                    Some(Group::Held) => SquareState::Held,
                    _ => SquareState::Waiting,
                }
            };
            square(Id::from(&member.id), state)
        })
        .collect()
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

/// The plan's members and edges as the graph draws them, with live presses: a needs-you
/// member with an open question opens it; otherwise cards select and rows open the task.
pub fn graph_input(feed: &Feed, plan: &Id, detail: &PlanDetail) -> GraphInput {
    let layers = layers(detail);
    let members = detail
        .members
        .iter()
        .zip(layers)
        .map(|(member, layer)| {
            let id = Id::from(&member.id);
            let task = feed.board.task(&member.id);
            let question = open_question(feed, &member.id);
            let you = needs_you(feed, member);
            let hue = match task.map(|task| task.group) {
                Some(Group::NeedsYou) => Hue::You,
                Some(Group::Verifying) => Hue::Corral,
                Some(Group::Producing) => Hue::Worker,
                Some(Group::Held) => Hue::Held,
                _ if you => Hue::You,
                _ => Hue::Neutral,
            };
            let now = task.map_or_else(|| member.state.clone(), |task| task.now.clone());
            let stale = task.is_some_and(|task| task.now_code.contains("stale"));
            let (select, open) = match question {
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
                title: task.map_or_else(|| member.title.clone(), |task| task.title.clone()),
                now,
                rail: rail(member),
                hue,
                now_hue: if you {
                    Hue::You
                } else if stale {
                    Hue::Held
                } else {
                    Hue::Neutral
                },
                needs_you: you,
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
        WireState::Drafting
        | WireState::Reviewing
        | WireState::PlanFailed
        | WireState::Executing
        | WireState::Complete
        | WireState::Cancelled
        | WireState::Other => {
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
fn tracker_actions(
    id: &Id,
    row: Option<&WirePlan>,
    detail: &PlanDetail,
    phone: bool,
) -> Vec<Button> {
    let mut actions = vec![];
    if !phone {
        actions.push(Button::new(
            "Ask about this",
            Action::Live(LiveAction::Ask(
                Live::Plan(id.clone()),
                row.map(|row| row.rev),
            )),
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
    let members = graph_input(feed, id, detail);
    let selected = state
        .live
        .member_of(id)
        .filter(|member| detail.members.iter().any(|m| m.id == member.as_str()))
        .cloned()
        .or_else(|| detail.members.first().map(|member| Id::from(&member.id)));
    let landed = row.map_or_else(
        || {
            detail
                .members
                .iter()
                .filter(|member| rail(member)[3] == Mark::Done)
                .count()
        },
        |row| row.members_landed as usize,
    );
    let total = row.map_or(detail.members.len(), |row| row.members_total as usize);
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
    let members_label = format!("Members {landed}/{total} landed");
    let pipeline = if detail.state == WireState::Complete {
        pipeline_marks([
            ("Design".into(), Mark::Done),
            (members_label, Mark::Done),
            ("Aggregate verify".into(), Mark::Done),
            ("Final accept".into(), Mark::Done),
        ])
    } else {
        pipeline("Design", members_label, false)
    };
    let warning = match (detail.scenarios, detail.mapped) {
        (Some(all), Some(0)) if all > 0 => Some(format!(
            "{all} of {all} acceptance scenarios are not mapped to any member · final verify has no owner"
        )),
        (Some(all), Some(mapped)) if mapped < all => Some(format!(
            "{} of {all} acceptance scenarios are not mapped to any member",
            all - mapped
        )),
        _ => None,
    };
    // What blocks final acceptance: every member not yet done (all of them, from the detail;
    // the row lists at most four), then corrald's own reasons acceptance is refused.
    let unfinished = detail
        .members
        .iter()
        .filter(|member| !matches!(member.state.as_str(), "done" | "dropped"))
        .map(|member| {
            let you = members
                .members
                .iter()
                .any(|shown| shown.needs_you && shown.id == member.id.as_str());
            Blocker {
                mark: if you { Mark::Active } else { Mark::Na },
                hue: if you { Hue::You } else { Hue::Neutral },
                text: if you {
                    format!("{} waits on your answer", member.id)
                } else {
                    format!(
                        "{} required work is {}",
                        member.id,
                        member.state.replace('_', " ")
                    )
                },
            }
        });
    let reasons = detail.accept_blockers.iter().map(|text| Blocker {
        mark: Mark::Na,
        hue: Hue::Neutral,
        text: text.clone(),
    });
    let blockers: Vec<Blocker> = unfinished.chain(reasons).collect();
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
            chips: vec![state_chip(detail.state)],
            id: id.to_string(),
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
        pipeline,
        warning,
        warning_mark: Mark::Stale,
        members_heading: "Members · live state",
        members,
        blockers_heading: "Blocking final acceptance",
        blocker_count: blockers.len(),
        blockers,
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
    let published = matches!(detail, Detail::Executing(_));
    PhonePage {
        title: match &detail {
            Detail::Awaiting(_) => "Plan review".into(),
            Detail::Diff(_) => "Revision diff".into(),
            Detail::Document(_) => "Plan document".into(),
            _ => id.to_string(),
        },
        back_label: "Plans",
        back: Effect::Back,
        right: published.then(|| {
            Button::new(
                "Ask about this",
                Action::Live(LiveAction::Ask(
                    Live::Plan(id.clone()),
                    row.map(|row| row.rev),
                )),
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
