//! Daemon-mode plumbing for Plans: keeping a shown plan's detail (and its open diff) loaded and
//! current, the pushed phone page of a served plan, and the plan graph Now's panel embeds.

use corral_protocol::detail::DetailRef;
use corral_protocol::rows::PlanState as WireState;
use nsh_core::prelude::*;

use crate::model::act::{self, ActStatus};
use crate::model::hue::Hue;
use crate::model::plans::{self, Action, Button, GraphInput, Lens, LiveAction};
use crate::model::text::span;
use crate::model::{Form, Id};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::kit::notice::{error, notice, skeleton};
use crate::ui::kit::text::dim;

use super::{command, graph};

/// What keeps one served plan current: its row revision, whether its copy went stale (a
/// reconnect), the diff open on it, and whether a command about it lost to a newer revision.
#[derive(Debug, Clone, PartialEq)]
struct Want {
    plan: Id,
    rev: Option<u64>,
    stale: bool,
    diff: Option<DetailRef>,
    diff_stale: bool,
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
        stale: feed
            .detail(&DetailRef::Plan {
                id: plan.to_string(),
            })
            .is_some_and(|cache| cache.stale),
        diff_stale: diff
            .as_ref()
            .and_then(|target| feed.detail(target))
            .is_some_and(|cache| cache.stale),
        conflict: keys
            .iter()
            .any(|key| matches!(act::status(feed, key), Some(ActStatus::Conflict(_)))),
        diff,
        plan,
    })
}

/// Loads the plan (and its open diff) when first shown or gone stale, and revalidates it when
/// its row revision moves or a command about it met a newer revision. Never refetches on an
/// error by itself: the detail offers Retry.
fn keep_current(app: AppState, plan: impl Fn() -> Option<Id> + 'static) {
    let wanted = computed(move || {
        if !app.daemon() {
            return None;
        }
        plan().map(|id| want(app, id))
    });
    let last = stored(None::<Want>);
    effect(move || {
        let Some(now) = wanted.get() else {
            return;
        };
        let before = last.replace(Some(now.clone()));
        let target = DetailRef::Plan {
            id: now.plan.to_string(),
        };
        untracked(|| {
            let moved = before.as_ref().is_some_and(|before| {
                before.plan == now.plan
                    && (before.rev != now.rev || (!before.conflict && now.conflict))
            });
            if moved {
                app.refresh(target);
            } else {
                app.load(target);
            }
            if let Some(diff) = now.diff.clone() {
                app.load(diff);
            }
        });
    });
}

/// Keeps the desktop and terminal detail's plan current: the root selection, or the first plan.
pub(super) fn keep_selected_loaded(app: AppState) {
    keep_current(app, move || {
        let selection = app.selection();
        app.feed.with(|feed| plans::current_plan(&selection, feed))
    });
}

/// Keeps the published plans' details loaded so their list rows can show member strips;
/// revalidates one when its row revision moves.
pub(super) fn keep_rows_loaded(app: AppState) {
    let rows = computed(move || {
        if !app.daemon() {
            return vec![];
        }
        app.feed.with(|feed| {
            feed.plans
                .values()
                .filter(|row| matches!(row.state, WireState::Executing | WireState::Complete))
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
                let target = DetailRef::Plan { id: id.to_string() };
                if before.iter().any(|(was, old)| was == id && old != rev) {
                    app.refresh(target);
                } else {
                    app.load(target);
                }
            }
        });
    });
}

/// The pushed phone page of a served plan (`Live::Plan`): the same anatomy as the fixture's,
/// from corrald's plan; its detail is kept current while the page is up.
pub fn live_plan_phone(app: AppState, id: Id) -> PhonePage {
    let shown = id.clone();
    keep_current(app, move || Some(shown.clone()));
    let env = app.env();
    let page = app.plans.with(|state| {
        app.feed
            .with(|feed| plans::live_phone_page(state, &id, feed, env))
    });
    super::phone_page(app, page, Some(id))
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
    keep_current(app, move || Some(shown.clone()));
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
        GraphState::Loading => skeleton(app, &[(0.5, 12.0), (0.8, 54.0)]),
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
