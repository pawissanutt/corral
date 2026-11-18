//! corrald's plans in the shared descriptors. A plan is a parent task with children: the list
//! reads the plan rows (`feed.plans`, the parent with per-status child counts), the detail
//! reads `PlanDetail` (the parent, whose body is the document, and its children), and the
//! member graph lays the children out by their `depends_on` edges. Absent facts stay absent;
//! nothing here falls back to fixtures.

use std::cmp::Ordering;

use corral_protocol::detail::{DetailBody, DetailRef, PlanDetail};
use corral_protocol::rows::PlanRow;
use corral_types::Status;

use super::member::Member;
use super::{
    Action, Button, Chip, Detail, DocumentFold, GraphInput, GraphMember, Header, LiveAction,
    NoticeView, PhonePage, PlanView, State, live_story, plan_brief,
};
use crate::model::daemon::{DetailCache, Feed, Group, Link};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::projects::{ProjectFilter, project_chip};
use crate::model::text::span;
use crate::model::work::when;
use crate::model::{Id, Live, Selection};

/// `T-9` before `T-10`; ids without a number after those with one.
pub(super) fn by_id(a: &str, b: &str) -> Ordering {
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

pub(super) fn plan_filter(feed: &Feed, filter: &ProjectFilter) -> ProjectFilter {
    if feed.capable(corral_protocol::PROJECTS_V1) {
        filter.resolved(feed)
    } else {
        ProjectFilter::All
    }
}

pub(super) fn matches_project(row: &PlanRow, filter: &ProjectFilter) -> bool {
    filter.matches(row.parent.project.as_deref())
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

/// The cached detail of a plan.
fn plan_cache<'a>(feed: &'a Feed, id: &str) -> Option<&'a DetailCache> {
    feed.detail(&DetailRef::Plan { id: id.to_owned() })
}

/// The plan's served detail body, if one has arrived.
pub fn plan_detail<'a>(feed: &'a Feed, id: &str) -> Option<&'a PlanDetail> {
    match plan_cache(feed, id)?.body()? {
        DetailBody::Plan(detail) => Some(detail),
        _ => None,
    }
}

pub(super) fn topic_loading(feed: &Feed) -> bool {
    feed.topics.get("plans").is_none_or(|topic| topic.loading)
}

/// A list row's project chip, while the scope spans several projects.
pub(super) fn row_project(feed: &Feed, row: &PlanRow, filter: &ProjectFilter) -> Option<String> {
    if !feed.capable(corral_protocol::PROJECTS_V1) {
        return None;
    }
    project_chip(
        feed,
        &plan_filter(feed, filter),
        row.parent.project.as_deref(),
    )
}

/// Each member's dependency layer: the longest chain of prerequisites within the plan, from 0
/// (bounded, so a cycle cannot loop).
fn layers(children: &[corral_types::Task]) -> Vec<usize> {
    let index = |id: &str| children.iter().position(|child| child.id == id);
    let edges: Vec<(usize, usize)> = children
        .iter()
        .enumerate()
        .flat_map(|(to, child)| {
            child
                .depends_on
                .iter()
                .filter_map(move |dep| Some((index(dep)?, to)))
        })
        .collect();
    let mut depth = vec![0; children.len()];
    for _ in 0..children.len() {
        let mut changed = false;
        for &(from, to) in &edges {
            let deeper = (depth[from] + 1).min(children.len());
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

/// The plan's children and their dependency edges as the graph draws them, each with its named
/// rail: placed cards select the member, stacked ones open its task.
pub fn graph_input(feed: &Feed, plan: &Id, detail: &PlanDetail) -> GraphInput {
    let members = detail
        .children
        .iter()
        .zip(layers(&detail.children))
        .map(|(child, layer)| {
            let member = Member::of(feed, detail, child);
            let id = Id::new(member.id());
            GraphMember {
                title: member.title().to_owned(),
                stage: member.rail(),
                hue: match member.group() {
                    Some(Group::NeedsYou) => Hue::You,
                    Some(Group::Verifying) => Hue::Corral,
                    Some(Group::Producing) => Hue::Worker,
                    _ if member.done() => Hue::Ok,
                    _ => Hue::Neutral,
                },
                needs_you: member.needs_you(),
                layer,
                select: Action::Live(LiveAction::Member {
                    plan: plan.clone(),
                    member: id.clone(),
                }),
                open: Action::Live(LiveAction::Task(id.clone())),
                id,
            }
        })
        .collect();
    let edges = detail
        .children
        .iter()
        .flat_map(|child| {
            child
                .depends_on
                .iter()
                .filter(|dep| detail.children.iter().any(|other| other.id == **dep))
                .map(|dep| (Id::new(dep), Id::new(&child.id)))
        })
        .collect();
    GraphInput { members, edges }
}

/// The sentence under a header whose detail is the last copy received, if it is.
fn stale_line(cache: &DetailCache, feed: &Feed) -> Option<String> {
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

/// The chip of a plan's parent task status.
fn status_chip(status: Status) -> Chip {
    let hue = match status {
        Status::Blocked => Hue::You,
        Status::Running | Status::Review => Hue::Corral,
        Status::Done => Hue::Ok,
        _ => Hue::Neutral,
    };
    Chip {
        label: status.as_str().into(),
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

fn ask(id: &Id) -> Button {
    Button::new(
        "Ask about this",
        Action::Live(LiveAction::Ask {
            record: Live::Plan(id.clone()),
        }),
    )
}

/// The detail of one served plan, or what stands in for it.
pub fn live_detail(state: &State, plan: Option<&Id>, feed: &Feed, env: Env) -> Detail {
    let Some(id) = plan else {
        return if feed.plans.is_empty() {
            notice(
                "No plan to show".into(),
                "A task with children is a plan; plans appear here when corrald serves them."
                    .into(),
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
    let cache = plan_cache(feed, id);
    let Some(detail) = plan_detail(feed, id) else {
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
        if !feed.plans.contains_key(id.as_str())
            && !topic_loading(feed)
            && cache.is_none_or(|cache| !cache.loading)
        {
            return notice(
                format!("Plan {id} is not served"),
                "corrald does not list this task as a plan.".into(),
                Hue::Neutral,
                None,
            );
        }
        return Detail::Loading;
    };
    let stale = cache.and_then(|cache| stale_line(cache, feed));
    Detail::Plan(Box::new(plan_view(state, feed, id, detail, stale, env)))
}

fn plan_view(
    state: &State,
    feed: &Feed,
    id: &Id,
    detail: &PlanDetail,
    stale: Option<String>,
    env: Env,
) -> PlanView {
    let phone = env.form == Form::Phone;
    // The board's row is fresher than the plan's copy of its parent.
    let parent = feed.board.task(id).map_or(&detail.parent, |row| row.task());
    let selected = state
        .member_of(id)
        .filter(|member| {
            detail
                .children
                .iter()
                .any(|child| child.id == member.as_str())
        })
        .cloned();
    let updated = when(&parent.updated.to_string(), feed);
    let story = if phone {
        None
    } else {
        selected.as_ref().and_then(|member| {
            detail
                .children
                .iter()
                .find(|child| child.id == member.as_str())
                .map(|child| live_story::story(feed, id, detail, child))
        })
    };
    PlanView {
        header: Header {
            plan: id.clone(),
            chips: vec![status_chip(parent.status)],
            id: id.to_string(),
            project: parent.project.clone(),
            title: parent.title.clone(),
            meta: if updated.is_empty() {
                vec![]
            } else {
                vec![span(format!("updated {updated}")).dim()]
            },
            actions: if phone { vec![] } else { vec![ask(id)] },
            stale,
        },
        brief: plan_brief(feed, id, detail),
        document_fold: phone
            .then(|| fold(id, parent.body.trim(), state.documents.contains(id)))
            .flatten(),
        document: parent.body.trim().to_owned(),
        members_heading: "Members",
        members_hint: "Laid out by dependency: a member starts once the members it depends on are done.",
        members_empty: "No members yet. A task filed under this one becomes a member.",
        selected,
        story,
    }
}

/// The lines a phone shows of a long document before its toggle, and the length past which a
/// document folds at all: a few lines left over read better in place than behind a press.
const FOLD_AT: usize = 14;
const FOLD_PAST: usize = 20;

/// The opening of a long document: up to `FOLD_AT` lines, cut at the last paragraph break in
/// them, and before a code fence that would be left open.
fn fold(plan: &Id, document: &str, open: bool) -> Option<DocumentFold> {
    let lines: Vec<&str> = document.lines().collect();
    if lines.len() <= FOLD_PAST {
        return None;
    }
    let mut cut = lines[..FOLD_AT]
        .iter()
        .rposition(|line| line.trim().is_empty())
        .filter(|at| *at > 0)
        .unwrap_or(FOLD_AT);
    let fences: Vec<usize> = (0..cut)
        .filter(|at| lines[*at].trim_start().starts_with("```"))
        .collect();
    if fences.len() % 2 == 1
        && let Some(open_fence) = fences.last()
    {
        cut = (*open_fence).max(1);
    }
    let rest = lines.len() - cut;
    Some(DocumentFold {
        preview: lines[..cut].join("\n"),
        open,
        toggle: Button::new(
            if open {
                "Show the opening only".to_owned()
            } else {
                format!("Read the whole document · {rest} more lines")
            },
            Action::Live(LiveAction::Document(plan.clone())),
        )
        .quiet(),
    })
}

/// The pushed phone page of one served plan.
pub fn live_phone_page(state: &State, id: &Id, feed: &Feed, env: Env) -> PhonePage {
    PhonePage {
        title: id.to_string(),
        right: ask(id),
        right_label: "Ask the assistant about this plan",
        detail: live_detail(
            state,
            Some(id),
            feed,
            Env {
                form: Form::Phone,
                ..env
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::layers;
    use crate::model::daemon::bare_task;

    #[test]
    fn a_member_sits_one_layer_past_its_deepest_prerequisite_and_a_cycle_stays_bounded() {
        let at = jiff::Timestamp::UNIX_EPOCH;
        let task = |id: &str, deps: &[&str]| {
            let mut task = bare_task(id, id, corral_types::Status::Ready, at);
            task.depends_on = deps.iter().map(|dep| (*dep).to_owned()).collect();
            task
        };
        let plan = [
            task("T-1", &[]),
            task("T-2", &["T-1"]),
            task("T-3", &["T-1", "T-2", "T-99"]),
        ];
        assert_eq!(layers(&plan), vec![0, 1, 2]);
        let cycle = [task("T-1", &["T-2"]), task("T-2", &["T-1"])];
        assert!(layers(&cycle).iter().all(|layer| *layer <= cycle.len()));
    }
}
