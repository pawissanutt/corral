//! The task page's Changes tab: what the worker said at `done`, then the stored diff file by
//! file, each patch read once it is opened. The diff is the review model's (`Tab::Diff` of the
//! task's candidate), split side by side only where the page's width allows (about 1100 pt of
//! content), unified below that, on a phone and in a terminal.

use nsh_core::prelude::*;

use crate::model::review::{Body, LoopContent, Tab};
use crate::model::tasks::page;
use crate::model::{Form, Id};
use crate::state::AppState;
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

/// The Changes tab of task `id`.
pub(crate) fn changes(app: AppState, id: Id) -> AnyElement {
    let form = app.form;
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
    let body = view()
        .test_id("task-changes")
        .on_layout(move |frame: &Bounds<Points>| measured.measured(frame.size.width.0))
        .child(summary)
        .child(block(app, page::DIFF, None, files));
    match form {
        Form::Terminal => body.child(term::blank()).into_element(),
        Form::Desktop => body.py(px(16.0)).into_element(),
        Form::Phone => body.px(px(14.0)).py(px(14.0)).into_element(),
    }
}
