//! The task page's Changes tab: what the worker said at `done`, then the stored diff file by
//! file, each patch read once it is opened. The diff is the review model's (`Tab::Diff` of the
//! task's candidate), split side by side only where the page's width allows (about 1100 pt of
//! content), unified below that, on a phone and in a terminal.

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::review::impact::expected::{self, Verdict};
use crate::model::review::impact::{Action, View};
use crate::model::review::{Body, LoopContent, Tab};
use crate::model::tasks::page;
use crate::model::{Form, Id, TaskTab};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button, seg_compact};
use crate::ui::kit::text::{dim, plain};
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
    // It shares the tab row, so on the phone it keeps to its words' width.
    seg_compact(
        app,
        &View::ALL,
        move || app.impact.with(|state| state.view),
        move |view| app.impact_act(id.as_str(), Action::View(view)),
        "seg-changes",
    )
}

fn content(app: AppState, id: Id, showing: View) -> AnyElement {
    let task = id.to_string();
    // A link's expectation may be set after the page is built; the guard follows it.
    let linked = computed(move || {
        app.impact
            .with(|state| state.expect.is_some() && state.task.as_deref() == Some(task.as_str()))
    });
    dynamic(move || {
        let id = id.clone();
        let linked = linked.get();
        untracked(move || {
            if linked {
                linked_content(app, id, showing)
            } else {
                unlinked(app, id, showing)
            }
        })
    })
    .into_element()
}

fn unlinked(app: AppState, id: Id, showing: View) -> AnyElement {
    match showing {
        View::Diff => diff_view(app, id, false),
        View::Impact => super::impact::impact(app, id),
    }
}

/// Changes opened by a diagram comparison's link: drawn only while every detail they draw from
/// names the linked submission; else why not, with a way to the current one at all times.
fn linked_content(app: AppState, id: Id, showing: View) -> AnyElement {
    let task = id.to_string();
    // The root detail is kept loaded here, since nothing is drawn until it answers.
    let roots = {
        let task = task.clone();
        computed(move || vec![expected::root(&task, showing)])
    };
    super::keep::keep_details(app, roots, None);
    let verdict = {
        let task = task.clone();
        computed(move || {
            let (expect, levels) = app.impact.with(|state| {
                let expect = state
                    .expect
                    .clone()
                    .filter(|_| state.task.as_deref() == Some(task.as_str()));
                (expect, state.map.trail.clone())
            });
            let expect = expect?;
            let patches: Vec<String> = app.review.with(|state| {
                state
                    .expanded
                    .iter()
                    .filter(|(at, _)| at.as_str() == task)
                    .map(|(_, path)| path.clone())
                    .collect()
            });
            let drawn = expected::Drawn {
                patches: &patches,
                levels: &levels,
            };
            let verdict = app
                .feed
                .with(|feed| expected::verdict(feed, &task, showing, &expect, drawn));
            Some((expect, verdict))
        })
    };
    expand_linked(app, id.clone(), showing, verdict);
    // Drawn only once confirmed, and torn down the moment a part names another submission.
    let state = computed(move || match verdict.get() {
        // Let go: `content` draws the unlinked view in this one's place.
        None => None,
        Some((_, Verdict::Same)) => Some(Ok(true)),
        Some((_, Verdict::Waiting)) => Some(Ok(false)),
        Some((_, Verdict::Other(why))) => Some(Err(why)),
    });
    let note = computed(move || verdict.get().map(|(expect, v)| expected::note(&expect, &v)));
    let current = {
        let id = id.clone();
        move || {
            let id = id.clone();
            button(
                app,
                "Show the current submission",
                ButtonKind::Ghost,
                "btn-changes-current",
                move || app.impact_act(id.as_str(), Action::Expect(None)),
            )
        }
    };
    dynamic(move || {
        let id = id.clone();
        let current = current.clone();
        let shown = state.get();
        untracked(move || match shown {
            Some(Err(why)) => view()
                .test_id("changes-expected-refused")
                .gap(px(8.0))
                .child(plain(app, why))
                .child(current())
                .into_element(),
            shown => {
                let body = match (shown, showing) {
                    (Some(Ok(true)), View::Diff) => Some(diff_view(app, id, true)),
                    (Some(Ok(true)), View::Impact) => Some(super::impact::impact(app, id)),
                    // Unconfirmed: nothing that could belong to another submission is drawn.
                    _ => None,
                };
                view()
                    .when(showing == View::Impact, |v| {
                        v.flex_1().min_h(px(0.0)).flex_col()
                    })
                    .child(
                        view()
                            .test_id("changes-expected")
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.0))
                            .child(dynamic(move || {
                                note.get().map(|words| dim(app, words).into_element())
                            }))
                            .child(current()),
                    )
                    .children(body)
                    .into_element()
            }
        })
    })
    .into_element()
}

/// Opens the linked file once the linked submission shows and the task is loaded, however cold.
fn expand_linked(
    app: AppState,
    id: Id,
    showing: View,
    verdict: Memo<Option<(expected::Expected, Verdict)>>,
) {
    let ready = computed(move || {
        let (expect, verdict) = verdict.get()?;
        let path = expect
            .path
            .filter(|_| showing == View::Diff && verdict == Verdict::Same)?;
        app.feed
            .with(|feed| crate::model::tasks::task_at(feed, id.as_str()).is_some())
            .then_some((id.clone(), path))
    });
    effect(move || {
        let Some((id, path)) = ready.get() else {
            return;
        };
        let filter = app.scope_filter();
        let focus = id.clone();
        app.reduce(app.review, move |state, feed, selection, _| {
            state.apply_live_filtered(
                crate::model::review::Action::Expand(path),
                feed,
                selection,
                Some(&focus),
                &filter,
            )
        });
        app.impact_act(id.as_str(), Action::Expanded);
    });
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

/// The stacked Changes tab on a phone or in a terminal; its switch is in the tab row.
pub(super) fn stacked(app: AppState, id: Id) -> AnyElement {
    let showing = computed(move || app.impact.with(|state| state.view));
    let content = dynamic(move || {
        let showing = showing.get();
        untracked(|| content(app, id.clone(), showing))
    });
    if app.form == Form::Terminal {
        view()
            .child(term::blank())
            .child(content)
            .child(term::blank())
            .into_element()
    } else {
        view()
            .px(px(14.0))
            .py(px(14.0))
            .child(content)
            .into_element()
    }
}

/// The worker's summary and the stored diff.
/// Linked to a submission, the summary is that submission's run's, from the review checked.
fn diff_view(app: AppState, id: Id, linked: bool) -> AnyElement {
    let summary = {
        let id = id.clone();
        computed(move || {
            app.feed.with(|feed| {
                if linked {
                    expected::summary(feed, id.as_str())
                } else {
                    page::summary(feed, id.as_str())
                }
            })
        })
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
