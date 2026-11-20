//! The task page's Overview: Description (the body's Markdown), Children (a plan's), Depends
//! on, then Pipeline, Checks (each round's gate and review), Runs (each opening its session,
//! the live reviewer's included) and Artifacts (the finder, narrowed to the task). On a wide
//! desktop what is read (Description, Checks with the reviewer's notes, Children, Depends on)
//! stands beside a rail of what is placed (Pipeline, Runs, Artifacts); elsewhere one column.

use nsh_core::prelude::*;

use crate::model::review::{ReadyLink, Rounds};
use crate::model::tasks::{self, page};
use crate::model::work::Press;
use crate::model::{Form, Id};
use crate::screens::markdown::{markdown, provide_styles};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::more::press;
use crate::ui::kit::status::pipeline_rows;
use crate::ui::term;
use crate::ui::tokens::MEASURE_PROSE;

use super::candidate::brief::chain;
use super::parts::{block, linked, plain_row, quiet, run};

/// The least width of a desktop page whose blocks stand in two columns.
const TWO_COLUMNS: f32 = 900.0;
/// How much faster the main column grows than the rail, and the rail's least and most width:
/// the rail grows with the page but the longest text (the description, the reviewer's notes)
/// keeps the larger share.
const MAIN_SHARE: f32 = 1.6;
const RAIL_MIN: f32 = 340.0;
const RAIL_MAX: f32 = 520.0;

/// The Overview of task `id`.
pub(crate) fn overview(app: AppState, id: Id) -> AnyElement {
    let form = app.form;
    let width = signal(0.0_f32);
    let wide = computed(move || form == Form::Desktop && width.get() >= TWO_COLUMNS);
    let body = dynamic(move || {
        let wide = wide.get();
        let id = id.clone();
        untracked(move || {
            let description = description(app, id.clone());
            let children = children(app, id.clone());
            let depends = depends(app, id.clone());
            let checks = checks(app, id.clone());
            let runs = runs(app, id.clone());
            let artifacts = artifacts(app, id.clone());
            if wide {
                return view()
                    .flex_row()
                    .gap(px(28.0))
                    .child(
                        view()
                            .flex_1()
                            .flex_grow(MAIN_SHARE)
                            .min_w(px(0.0))
                            .child(description)
                            .child(checks)
                            .child(children)
                            .child(depends),
                    )
                    .child(
                        view()
                            .flex_1()
                            .min_w(px(RAIL_MIN))
                            .max_w(px(RAIL_MAX))
                            .child(pipeline(app, id))
                            .child(runs)
                            .child(artifacts),
                    )
                    .into_element();
            }
            view()
                .child(description)
                .child(children)
                .child(depends)
                .children((form != Form::Phone).then(|| pipeline(app, id)))
                .child(checks)
                .child(runs)
                .child(artifacts)
                .into_element()
        })
    });
    let page = view()
        .test_id("task-overview")
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .child(body);
    match form {
        Form::Terminal => page.child(term::blank()).into_element(),
        Form::Desktop => page.py(px(16.0)).into_element(),
        Form::Phone => page.px(px(18.0)).py(px(14.0)).into_element(),
    }
}

/// The body's Markdown, exactly as the engine stores it, kept to a readable line on desktop.
fn description(app: AppState, id: Id) -> AnyElement {
    let body = computed(move || {
        app.feed
            .with(|feed| tasks::facts(feed, id.as_str()).and_then(|facts| facts.body))
    });
    provide_styles(app);
    dynamic(move || {
        let content = match body.get() {
            Some(body) => view()
                .when(app.form == Form::Desktop, |prose| {
                    prose.max_w(px(MEASURE_PROSE))
                })
                .child(markdown(app, &body, false))
                .into_element(),
            None => quiet(app, page::NO_DESCRIPTION),
        };
        block(app, page::DESCRIPTION, None, content)
    })
}

/// A plan's children; nothing for a task without any.
fn children(app: AppState, id: Id) -> AnyElement {
    let list = computed(move || app.feed.with(|feed| page::children(feed, id.as_str())));
    dynamic(move || {
        let list = list.get();
        (!list.is_empty()).then(|| {
            let rows: Vec<AnyElement> = list.iter().map(|child| linked(app, child)).collect();
            block(
                app,
                page::CHILDREN,
                Some(list.len().to_string()),
                view().children(rows).into_element(),
            )
        })
    })
}

/// What the task depends on; nothing for a task that depends on none.
fn depends(app: AppState, id: Id) -> AnyElement {
    let list = computed(move || app.feed.with(|feed| page::depends(feed, id.as_str())));
    dynamic(move || {
        let list = list.get();
        (!list.is_empty()).then(|| {
            let rows: Vec<AnyElement> = list.iter().map(|task| linked(app, task)).collect();
            block(
                app,
                page::DEPENDS,
                None,
                view().children(rows).into_element(),
            )
        })
    })
}

/// Run, gate, review and land, spelled out.
pub(crate) fn pipeline(app: AppState, id: Id) -> AnyElement {
    let steps = computed(move || {
        app.feed
            .with(|feed| tasks::line(feed, id.as_str()).map(|line| line.pipeline))
    });
    dynamic(move || {
        steps.get().map(|steps| {
            block(
                app,
                page::PIPELINE,
                None,
                view()
                    .test_id("pipeline")
                    .child(pipeline_rows(app, &steps))
                    .into_element(),
            )
        })
    })
}

/// The newest gate and review in the readiness chain's words, then each earlier round.
fn checks(app: AppState, id: Id) -> AnyElement {
    let model = computed(move || {
        app.feed.with(|feed| {
            tasks::task_at(feed, id.as_str())
                .map(|task| crate::model::review::checks(feed, &task))
                .unwrap_or_default()
        })
    });
    dynamic(move || {
        let (links, rounds): (Vec<ReadyLink>, Option<Rounds>) = model.get();
        let note = rounds.as_ref().and_then(|rounds| rounds.count.clone());
        let earlier: Vec<AnyElement> = rounds
            .iter()
            .flat_map(|rounds| rounds.rounds.iter())
            .filter(|round| !round.current)
            .map(|round| {
                view()
                    .pt(px(if app.form == Form::Terminal { 0.0 } else { 8.0 }))
                    .child(quiet(app, round.title.clone()))
                    .child(chain(app, &round.links))
                    .into_element()
            })
            .collect();
        let content = if links.is_empty() && earlier.is_empty() {
            quiet(app, page::NO_CHECKS)
        } else {
            view()
                .child(chain(app, &links))
                .children(earlier)
                .into_element()
        };
        block(app, page::CHECKS, note, content)
    })
}

/// The task's runs, each opening its session.
fn runs(app: AppState, id: Id) -> AnyElement {
    let list = computed(move || app.feed.with(|feed| page::runs(feed, id.as_str())));
    dynamic(move || {
        let list = list.get();
        let content = if list.is_empty() {
            quiet(app, page::NO_RUNS)
        } else {
            let rows: Vec<AnyElement> = list.iter().map(|line| run(app, line)).collect();
            view().children(rows).into_element()
        };
        block(app, page::RUNS, None, content)
    })
}

/// The task's artifacts, then the finder narrowed to the task.
fn artifacts(app: AppState, id: Id) -> AnyElement {
    let list = {
        let id = id.clone();
        computed(move || app.feed.with(|feed| page::artifacts(feed, id.as_str())))
    };
    dynamic(move || {
        let list = list.get();
        let id = id.clone();
        let find = button(
            app,
            page::FIND,
            ButtonKind::Ghost,
            "btn-find-artifacts",
            move || {
                press(app, Press::FindArtifacts(id.clone()));
            },
        );
        let rows: Vec<AnyElement> = if list.is_empty() {
            vec![quiet(app, page::NO_ARTIFACTS)]
        } else {
            list.iter()
                .map(|line| plain_row(app, line.kind.clone(), line.title.clone(), line.at.clone()))
                .collect()
        };
        let content = view()
            .children(rows)
            .child(
                view()
                    .flex_row()
                    .pt(px(if app.form == Form::Terminal { 0.0 } else { 8.0 }))
                    .child(find),
            )
            .into_element();
        block(app, page::ARTIFACTS, None, content)
    })
}
