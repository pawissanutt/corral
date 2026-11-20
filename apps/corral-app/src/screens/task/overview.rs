//! The task page's Overview: Description (the body's Markdown), Children (a plan's), Depends
//! on, then Pipeline, Checks (each round's gate and review), Runs (each opening its session,
//! the live reviewer's included) and Artifacts (the finder, narrowed to the task). On a wide
//! desktop the first three stand beside the rest; elsewhere one column.

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

use super::candidate::brief::chain;
use super::parts::{block, linked, plain_row, quiet, run};

/// The least width of a desktop page whose blocks stand in two columns.
const TWO_COLUMNS: f32 = 900.0;

/// The Overview of task `id`.
pub(crate) fn overview(app: AppState, id: Id) -> AnyElement {
    let form = app.form;
    let width = signal(0.0_f32);
    let wide = computed(move || form == Form::Desktop && width.get() >= TWO_COLUMNS);
    let body = dynamic(move || {
        let wide = wide.get();
        let id = id.clone();
        untracked(move || {
            let left = vec![
                description(app, id.clone()),
                children(app, id.clone()),
                depends(app, id.clone()),
            ];
            let mut right = Vec::new();
            if form != Form::Phone {
                right.push(pipeline(app, id.clone()));
            }
            right.extend([
                checks(app, id.clone()),
                runs(app, id.clone()),
                artifacts(app, id),
            ]);
            if wide {
                view()
                    .flex_row()
                    .gap(px(28.0))
                    .child(view().flex_1().min_w(px(0.0)).children(left))
                    .child(view().w(px(380.0)).flex_shrink_0().children(right))
                    .into_element()
            } else {
                view().children(left).children(right).into_element()
            }
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

/// The body's Markdown, exactly as the engine stores it.
fn description(app: AppState, id: Id) -> AnyElement {
    let body = computed(move || {
        app.feed
            .with(|feed| tasks::facts(feed, id.as_str()).and_then(|facts| facts.body))
    });
    provide_styles(app);
    dynamic(move || {
        let content = match body.get() {
            Some(body) => markdown(app, &body, false),
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
