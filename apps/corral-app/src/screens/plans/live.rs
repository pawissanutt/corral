//! Plans plumbing over the served feed: keeping a shown plan's detail loaded, current and pinned
//! in the client's cache, the pushed phone page of a plan, and the plan graph Now's panel
//! embeds.

use corral_protocol::detail::DetailRef;
use corral_protocol::rows::PlanRow;
use nsh_core::prelude::*;

use crate::model::daemon::Feed;
use crate::model::hue::Hue;
use crate::model::plans::{self, Action, Button, GraphInput, LiveAction};
use crate::model::projects::FilterScreen;
use crate::model::text::span;
use crate::model::{Form, Id};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::kit::notice::{error, loading, notice};
use crate::ui::kit::text::dim;

use super::{command, graph};

/// What keeps one served plan current: its row (the parent and its child counts, which move
/// whenever a member does) and whether the cache's copy failed.
#[derive(Debug, Clone, PartialEq)]
struct Want {
    plan: Id,
    row: Option<PlanRow>,
    failed: bool,
}

fn plan_ref(plan: &Id) -> DetailRef {
    DetailRef::Plan {
        id: plan.to_string(),
    }
}

fn want(feed: &Feed, plan: Id) -> Want {
    Want {
        row: feed.plans.get(plan.as_str()).cloned(),
        failed: feed
            .detail(&plan_ref(&plan))
            .is_some_and(|cache| cache.error.is_some() && !cache.loading),
        plan,
    }
}

/// Loads the plan when first shown or dropped from the cache, and revalidates it when its row
/// moves. A failed request is fetched again only when the plan is shown anew: the detail offers
/// Retry. While shown, the detail is pinned, so the cache keeps it over older details. Requests
/// and pins go to the source the view mounted on (`AppState::scope`).
fn keep_current(app: AppState, plan: impl Fn() -> Option<Id> + 'static) {
    let app = untracked(|| app.scope());
    let wanted = computed(move || {
        if !app.daemon() {
            return None;
        }
        let id = plan()?;
        Some(app.feed.with(|feed| want(feed, id)))
    });
    let last = stored(None::<Want>);
    let pinned = stored(None::<DetailRef>);
    effect(move || {
        let Some(now) = wanted.get() else {
            return;
        };
        let before = last.replace(Some(now.clone()));
        let target = plan_ref(&now.plan);
        untracked(|| {
            let fresh = before.as_ref().is_none_or(|before| before.plan != now.plan);
            if pinned.with(|pinned| pinned.as_ref() != Some(&target)) {
                if let Some(gone) = pinned.replace(Some(target.clone())) {
                    app.watch_detail(gone, false);
                }
                app.watch_detail(target.clone(), true);
            }
            let moved = !fresh && before.as_ref().is_some_and(|before| before.row != now.row);
            if moved {
                app.refresh(target);
            } else if fresh || !now.failed {
                app.load(target);
            }
        });
    });
    // The owner may already be letting go of its values when this runs.
    cleanup(move || {
        if let Some(gone) = pinned.try_with(Option::take).flatten() {
            app.watch_detail(gone, false);
        }
    });
}

/// Keeps the desktop and terminal detail's plan current: the root selection, or the first plan
/// the Plans scope lists.
pub(super) fn keep_selected_loaded(app: AppState) {
    keep_current(app, move || {
        let selection = app.selection();
        let filter = app.project_filter(FilterScreen::Plans);
        app.feed
            .with(|feed| plans::current_plan_filtered(&selection, feed, &filter))
    });
}

/// The pushed phone page of a plan (`Live::Plan`); its detail is kept current while the page
/// is up.
pub fn live_plan_phone(app: AppState, id: Id) -> PhonePage {
    let shown = id.clone();
    keep_current(app, move || Some(shown.clone()));
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
/// itself, rings `member`, and opens a member's task on press.
pub fn live_graph(app: AppState, plan: Id, member: Option<Id>) -> AnyElement {
    let shown = plan.clone();
    keep_current(app, move || Some(shown.clone()));
    let target = plan_ref(&plan);
    let input = {
        let plan = plan.clone();
        computed(move || {
            app.feed.with(|feed| {
                plans::plan_detail(feed, &plan).map(|detail| {
                    let mut input = plans::graph_input(feed, &plan, detail);
                    // In another destination a card opens its member rather than selecting it
                    // in a Plans detail that is not on screen.
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
        GraphState::Empty => dim(app, "This plan has no members yet.").into_element(),
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
