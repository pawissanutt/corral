//! Plans plumbing over the served feed: keeping a shown plan's detail (and its open diff)
//! loaded, current and pinned in the client's cache, the pushed phone page of a plan, and the
//! plan graph Now's panel embeds.

use corral_protocol::detail::DetailRef;
use corral_protocol::rows::PlanState as WireState;
use nsh_core::prelude::*;

use crate::model::act::{self, ActStatus};
use crate::model::daemon::Feed;
use crate::model::hue::Hue;
use crate::model::plans::{self, Action, Button, GraphInput, Lens, LiveAction};
use crate::model::text::span;
use crate::model::work;
use crate::model::{Form, Id};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::kit::notice::{error, loading, notice};
use crate::ui::kit::text::dim;

use super::{command, graph};

/// What the client's cache holds of a detail. Missing covers a copy the cache let go of, so a
/// shown plan whose copy was dropped is asked for again rather than left loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cached {
    Missing,
    Loading,
    Current,
    Stale,
    Failed,
}

fn cached(feed: &Feed, target: &DetailRef) -> Cached {
    match feed.detail(target) {
        None => Cached::Missing,
        Some(cache) if cache.loading => Cached::Loading,
        Some(cache) if cache.error.is_some() => Cached::Failed,
        Some(cache) if cache.stale => Cached::Stale,
        Some(_) => Cached::Current,
    }
}

/// What keeps one served plan current: its row revision, what the cache holds of it, of its
/// task (whose owner revision the plan's commands carry) and of the diff open on it, and whether
/// a command about it lost to a newer revision.
#[derive(Debug, Clone, PartialEq)]
struct Want {
    plan: Id,
    rev: Option<u64>,
    cached: Cached,
    task_cached: Cached,
    diff: Option<DetailRef>,
    diff_cached: Cached,
    conflict: bool,
}

fn want(app: AppState, plan: Id) -> Want {
    let (lens, keys) = app.plans.with(|state| {
        let keys: Vec<Id> = [&state.live.accept, &state.live.changes]
            .into_iter()
            .flatten()
            .filter(|sent| sent.plan == plan)
            .map(|sent| sent.key.clone())
            .collect();
        (state.live.lens_of(&plan).cloned(), keys)
    });
    let diff = match lens {
        Some(Lens::Diff { from, to }) => Some(DetailRef::PlanDiff {
            id: plan.to_string(),
            from,
            to,
        }),
        _ => None,
    };
    app.feed.with(|feed| Want {
        rev: feed.plans.get(plan.as_str()).map(|row| row.rev),
        cached: cached(feed, &plan_ref(&plan)),
        task_cached: cached(feed, &work::task_ref(&plan)),
        diff_cached: diff
            .as_ref()
            .map_or(Cached::Missing, |target| cached(feed, target)),
        conflict: keys
            .iter()
            .any(|key| matches!(act::status(feed, key), Some(ActStatus::Conflict(_)))),
        diff,
        plan,
    })
}

fn plan_ref(plan: &Id) -> DetailRef {
    DetailRef::Plan {
        id: plan.to_string(),
    }
}

/// Loads the plan (and its open diff) when first shown, gone stale or dropped from the cache,
/// and revalidates it when its row revision moves or a command about it met a newer revision.
/// With `commands`, the plan's task detail too, so the plan's verbs can carry its canonical
/// owner revision (RC-03). A failed request is fetched again only when the plan is shown anew:
/// the detail offers Retry. While shown, what it loads is pinned, so the cache keeps it over
/// older details. Requests and pins go to the source the view mounted on (`AppState::scope`).
fn keep_current(app: AppState, commands: bool, plan: impl Fn() -> Option<Id> + 'static) {
    let app = untracked(|| app.scope());
    let wanted = computed(move || {
        if !app.daemon() {
            return None;
        }
        plan().map(|id| want(app, id))
    });
    let last = stored(None::<Want>);
    let pinned = stored(Vec::<DetailRef>::new());
    effect(move || {
        let Some(now) = wanted.get() else {
            return;
        };
        let before = last.replace(Some(now.clone()));
        let target = plan_ref(&now.plan);
        let task = commands.then(|| work::task_ref(&now.plan));
        untracked(|| {
            let fresh = before.as_ref().is_none_or(|before| before.plan != now.plan);
            let moved = !fresh
                && before.as_ref().is_some_and(|before| {
                    before.rev != now.rev || (!before.conflict && now.conflict)
                });
            pin(
                app,
                pinned,
                [Some(target.clone()), task.clone(), now.diff.clone()],
            );
            if moved {
                app.refresh(target);
            } else if fresh || now.cached != Cached::Failed {
                app.load(target);
            }
            if let Some(task) = task {
                if moved {
                    app.refresh(task);
                } else if fresh || now.task_cached != Cached::Failed {
                    app.load(task);
                }
            }
            if let Some(diff) = now.diff.clone()
                && (fresh || now.diff_cached != Cached::Failed)
            {
                app.load(diff);
            }
        });
    });
    // The owner may already be letting go of its values when this runs.
    cleanup(move || {
        for gone in pinned.try_with(std::mem::take).unwrap_or_default() {
            app.watch_detail(gone, false);
        }
    });
}

/// Pins `wanted` in the client's cache and lets go of what was pinned before and no longer is.
fn pin(app: AppState, pinned: Stored<Vec<DetailRef>>, wanted: [Option<DetailRef>; 3]) {
    let wanted: Vec<DetailRef> = wanted.into_iter().flatten().collect();
    let was = pinned.replace(wanted.clone());
    for gone in was.iter().filter(|target| !wanted.contains(target)) {
        app.watch_detail(gone.clone(), false);
    }
    for new in wanted.iter().filter(|target| !was.contains(target)) {
        app.watch_detail(new.clone(), true);
    }
}

/// Keeps the desktop and terminal detail's plan current: the root selection, or the first plan.
pub(super) fn keep_selected_loaded(app: AppState) {
    keep_current(app, true, move || {
        let selection = app.selection();
        app.feed.with(|feed| plans::current_plan(&selection, feed))
    });
}

/// Keeps the executing plans' details loaded so their list rows can show member strips;
/// revalidates one when its row revision moves. A complete plan's members have all landed, and
/// a long history of them would crowd the bounded cache, so its detail loads when it is opened.
/// Requests go to the source the view mounted on (`AppState::scope`).
pub(super) fn keep_rows_loaded(app: AppState) {
    let app = untracked(|| app.scope());
    let rows = computed(move || {
        if !app.daemon() {
            return vec![];
        }
        app.feed.with(|feed| {
            feed.plans
                .values()
                .filter(|row| matches!(row.state, WireState::Executing))
                .map(|row| (Id::from(&row.id), row.rev))
                .collect::<Vec<_>>()
        })
    });
    let last = stored(Vec::<(Id, u64)>::new());
    effect(move || {
        let now = rows.get();
        let before = last.replace(now.clone());
        untracked(|| {
            for (id, rev) in &now {
                let target = plan_ref(id);
                if before.iter().any(|(was, old)| was == id && old != rev) {
                    app.refresh(target);
                } else {
                    app.load(target);
                }
            }
        });
    });
}

/// The pushed phone page of a plan (`Live::Plan`); its detail is kept current while the page
/// is up.
pub fn live_plan_phone(app: AppState, id: Id) -> PhonePage {
    let shown = id.clone();
    keep_current(app, true, move || Some(shown.clone()));
    let env = app.env();
    let page = app.plans.with(|state| {
        app.feed
            .with(|feed| plans::live_phone_page(state, &id, feed, env))
    });
    super::phone_page(app, page, id)
}

/// What the embedded graph shows: the plan's members, or what stands in for them.
#[derive(Debug, Clone, PartialEq)]
enum GraphState {
    Ready { stale: Option<String> },
    Empty,
    Loading,
    Failed(String),
}

/// A served plan's member graph alone, for Now's plan panel: loads and keeps the plan current
/// itself, rings `member`, and opens a member's task (or its open question) on press.
pub fn live_graph(app: AppState, plan: Id, member: Option<Id>) -> AnyElement {
    let shown = plan.clone();
    // Now's panel offers no plan commands, so the plan's task detail is not needed here.
    keep_current(app, false, move || Some(shown.clone()));
    let target = DetailRef::Plan {
        id: plan.to_string(),
    };
    let input = {
        let plan = plan.clone();
        computed(move || {
            app.feed.with(|feed| {
                plans::plan_detail(feed, &plan).map(|detail| {
                    let mut input = plans::graph_input(feed, &plan, detail);
                    // In another destination a card opens its member rather than selecting it
                    // in a Plans tracker that is not on screen.
                    for member in &mut input.members {
                        member.select = member.open.clone();
                    }
                    input
                })
            })
        })
    };
    let state = computed(move || {
        let stale = app.with_detail(&target, |cache| {
            cache.and_then(|cache| {
                cache
                    .stale
                    .then(|| "Showing the last plan received from corrald.".to_owned())
            })
        });
        let failed = app.with_detail(&target, |cache| {
            cache
                .filter(|cache| !cache.loading)
                .and_then(|cache| cache.error.as_ref().map(|error| error.message.clone()))
        });
        input.with(|input| match (input, failed) {
            (Some(input), _) if input.members.is_empty() => GraphState::Empty,
            (Some(_), _) => GraphState::Ready { stale },
            (None, Some(why)) => GraphState::Failed(why),
            (None, None) => GraphState::Loading,
        })
    });
    dynamic(move || match state.get() {
        GraphState::Ready { stale } => view()
            .children(stale.map(|line| {
                view()
                    .pb(px(if app.form == Form::Terminal { 0.0 } else { 8.0 }))
                    .child(notice(app, Hue::Held, &vec![span(line)], None))
                    .into_element()
            }))
            .child(graph::drawing(app, {
                let member = member.clone();
                move || {
                    (
                        input.with(|input| input.clone().unwrap_or_else(GraphInput::default)),
                        member.clone(),
                    )
                }
            }))
            .into_element(),
        GraphState::Empty => dim(app, "No members are published for this plan yet.").into_element(),
        GraphState::Loading => loading(app, plans::DETAIL_LOADING, &[(0.5, 12.0), (0.8, 54.0)]),
        GraphState::Failed(why) => error(
            app,
            &vec![span(format!("Could not load plan {plan}: {why}"))],
            Some(command(
                app,
                &Button::new("Retry", Action::Live(LiveAction::Retry(plan.clone()))),
                false,
            )),
        ),
    })
}
