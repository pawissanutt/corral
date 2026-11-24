//! The task page's Changes tab: what the worker said at `done`, then the stored diff file by
//! file, each patch read once it is opened. The diff is the review model's (`Tab::Diff` of the
//! task's candidate), split side by side only where the page's width allows (about 1100 pt of
//! content), unified below that, on a phone and in a terminal.

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::review::impact::{Action, View};
use crate::model::review::{Body, LoopContent, Tab};
use crate::model::tasks::page;
use crate::model::{Form, Id, TaskTab};
use crate::state::AppState;
use crate::ui::kit::button::seg;
use crate::ui::kit::text::plain;
use crate::ui::term;

use super::candidate::{Ctx, diff, layout_switch};
use super::parts::{block, quiet};

/// What the diff shows: the files, or why there are none.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Shown {
    Diff(crate::model::review::DiffView),
    Line(String),
}

/// Diff or Impact (`seg-changes-<n>`), in the desktop tab row or above the stacked body.
pub(super) fn view_switch(app: AppState, id: Id) -> AnyElement {
    seg(
        app,
        &View::ALL,
        move || app.impact.with(|state| state.view),
        move |view| app.impact_act(id.as_str(), Action::View(view)),
        "seg-changes",
    )
}

fn content(app: AppState, id: Id, showing: View) -> AnyElement {
    match showing {
        View::Diff => diff_view(app, id),
        View::Impact => super::impact::impact(app, id),
    }
}

/// The desktop map and its panes need the remaining page height, unlike the flowing Diff.
pub(super) fn desktop(app: AppState, id: Id, showing: View) -> AnyElement {
    let body = view().child(content(app, id, showing));
    match showing {
        View::Diff => body.py(px(16.0)).into_element(),
        View::Impact => body
            .flex_1()
            .min_h(px(0.0))
            .flex_col()
            .py(px(8.0))
            .into_element(),
    }
}

/// The stacked Changes tab on a phone or in a terminal.
pub(super) fn stacked(app: AppState, id: Id) -> AnyElement {
    let switch = view_switch(app, id.clone());
    let showing = computed(move || app.impact.with(|state| state.view));
    let content = dynamic(move || {
        let showing = showing.get();
        untracked(|| content(app, id.clone(), showing))
    });
    if app.form == Form::Terminal {
        view()
            .child(switch)
            .child(term::blank())
            .child(content)
            .child(term::blank())
            .into_element()
    } else {
        view()
            .px(px(14.0))
            .py(px(14.0))
            .gap(px(10.0))
            .child(switch)
            .child(content)
            .into_element()
    }
}

/// The worker's summary and the stored diff.
fn diff_view(app: AppState, id: Id) -> AnyElement {
    let summary = {
        let id = id.clone();
        computed(move || app.feed.with(|feed| page::summary(feed, id.as_str())))
    };
    let summary = dynamic(move || {
        let content = match summary.get() {
            Some(words) => plain(app, words).into_element(),
            None => quiet(app, page::NO_SUMMARY),
        };
        block(app, page::SUMMARY, None, content)
    });
    keep_closed_diff(app, id.clone());
    let cx = Ctx::showing(app, Some(id), Some(Tab::Diff));
    let page = cx.page;
    let split = cx.split;
    let shown = computed(move || {
        split.get()?;
        page.with(|page| {
            Some(match page.detail.as_ref().map(|detail| &detail.content) {
                Some(LoopContent::Evidence(Body::Diff(files))) => Shown::Diff(files.clone()),
                Some(LoopContent::Evidence(Body::Notice(line))) => Shown::Line(line.clone()),
                _ => Shown::Line(page.empty.clone().unwrap_or_default()),
            })
        })
    });
    let measured = cx.clone();
    let files = {
        let cx = cx.clone();
        dynamic(move || {
            let cx = cx.clone();
            shown.get().map(|shown| {
                untracked(move || match shown {
                    Shown::Diff(files) => view()
                        .child(layout_switch(&cx))
                        .child(diff::diff(&cx, &files))
                        .into_element(),
                    Shown::Line(line) => quiet(cx.app, line),
                })
            })
        })
    };
    view()
        .test_id("task-changes")
        .on_layout(move |frame: &Bounds<Points>| measured.measured(frame.size.width.0))
        .child(summary)
        .child(block(app, page::DIFF, None, files))
        .into_element()
}

/// Open tasks already have a candidate keeper. For a closed task either Changes view needs
/// the candidate to show the tab's file count.
pub(super) fn keep_closed_review(app: AppState, id: Id) {
    let targets = computed(move || {
        if app.nav.with(|nav| nav.task_tab) == TaskTab::Changes && closed(app, &id) {
            vec![DetailRef::Review {
                task: id.to_string(),
            }]
        } else {
            Vec::new()
        }
    });
    super::keep::keep_details(app, targets, None);
}

/// Only the Diff needs patches, and then only for its expanded files.
fn keep_closed_diff(app: AppState, id: Id) {
    let targets = computed(move || {
        if !closed(app, &id) {
            return Vec::new();
        }
        app.review.with(|state| {
            state
                .expanded
                .iter()
                .filter(|(task, _)| task == &id)
                .map(|(_, path)| DetailRef::ReviewFile {
                    task: id.to_string(),
                    path: path.clone(),
                })
                .collect()
        })
    });
    super::keep::keep_details(app, targets, None);
}

fn closed(app: AppState, id: &Id) -> bool {
    app.feed
        .with(|feed| feed.board.complete && feed.board.task(id.as_str()).is_none())
}
