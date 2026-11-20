//! The task page: the one page of a task on every form. A plan is a task with children and a
//! review a task in Review, so `Live::Plan` and `Live::Review` open it too. Its head goes back
//! to Tasks and names the task (status, id, title, Ask about this, More); a meta line gives
//! its project, priority, kind, size, base, profile and round; the decision band (`band.rs`)
//! holds what the owner decides; then the tabs Overview, Changes and History (`NavState::task_tab`, keys `1`
//! `2` `3`, in the address as `/task/<id>/<tab>`). Escape returns to Tasks. The phone shows the
//! same page in one column, the band first.
//!
//! While the page shows, it keeps the task's detail (and a plan's detail, and a candidate's
//! review detail) loaded and current.

mod band;
pub(crate) mod candidate;
mod changes;
mod history;
pub(crate) mod keep;
pub(crate) mod overview;
pub(crate) mod parts;

use nsh_core::prelude::*;

use crate::model::tasks::{self, page as task_page};
use crate::model::work::Record;
use crate::model::{Destination, Form, Id, Live, TaskTab};
use crate::screens::PhonePage;
use crate::screens::task::keep::{keep_loaded, wanted_id};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with, tab_strip};
use crate::ui::kit::next::more_button;
use crate::ui::kit::status::status_word_mark;
use crate::ui::kit::text::{dim, mono, plain};
use crate::ui::term;
use crate::ui::tokens::{Token, color};

pub(crate) use band::decision;

/// Ask opened with the task's bounded context (`btn-ask-about`), on its page and in the
/// inspector.
pub(crate) fn ask_about(app: AppState, id: &Id) -> AnyElement {
    let context = Live::Task(id.clone()).context(None);
    button_with(
        app,
        task_page::ASK,
        ButtonKind::Ghost,
        ButtonOptions {
            icon: Some(Icon::Ask),
            ..ButtonOptions::default()
        },
        "btn-ask-about",
        move || app.ask_about(Some(context.clone()), None),
    )
}

/// The page on desktop and in a terminal.
pub fn page(app: AppState, id: Id) -> AnyElement {
    let form = app.form;
    let back = button(
        app,
        format!("‹ {}", Destination::Tasks.label()),
        ButtonKind::Quiet,
        "btn-back",
        move || app.back(),
    );
    let ask = ask_about(app, &id);
    let more = more_button(app, Record::Task(id.clone()), false);
    let head = match form {
        Form::Terminal => view()
            .flex_none()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .h(term::rows(1.0))
                    .gap(px(8.0))
                    .child(back)
                    .child(named(app, &id))
                    .child(ask)
                    .child(more),
            )
            .child(view().px(term::cells(1.0)).child(meta(app, &id)))
            .child(term::hrule())
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .flex_none()
            .px(px(18.0))
            .pt(px(8.0))
            .pb(px(10.0))
            .gap(px(4.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .child(back)
                    .child(named(app, &id))
                    .child(ask)
                    .child(more),
            )
            .child(meta(app, &id))
            .into_element(),
    };
    let page = view()
        .test_id("task-page")
        .size_full()
        .flex_grow(1.0)
        .flex_col()
        .min_h(px(0.0))
        .bg(color(Token::Bg))
        .child(keeper(app, id.clone()))
        .child(head);
    match form {
        // 24 rows hold the head, a review's Onto line and decide bar and little else, so the
        // band and the tabs scroll with the tab's body there.
        Form::Terminal => page
            .child(
                scroll().flex_1().min_h(px(0.0)).child(
                    view()
                        .child(decision(app, id.clone(), false))
                        .child(view().px(term::cells(1.0)).child(tabs(app)))
                        .child(view().px(term::cells(1.0)).child(tab_body(app, id))),
                ),
            )
            .into_element(),
        Form::Desktop | Form::Phone => page
            .child(view().flex_none().child(decision(app, id.clone(), false)))
            .child(view().flex_none().px(px(18.0)).child(tabs(app)))
            .child(
                scroll()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(view().px(px(18.0)).child(tab_body(app, id))),
            )
            .into_element(),
    }
}

/// The page on a phone: the shell heads it with the task's id and More, and scrolls its body:
/// the title, meta line and Ask about this, the band, the tabs and the tab's body.
pub fn phone(app: AppState, id: Id) -> PhonePage {
    let body = view()
        .test_id("task-page")
        .child(keeper(app, id.clone()))
        .child(
            view()
                .px(px(18.0))
                .pt(px(4.0))
                .pb(px(10.0))
                .gap(px(6.0))
                .child(named(app, &id))
                .child(meta(app, &id))
                .child(view().flex_row().child(ask_about(app, &id))),
        )
        .child(decision(app, id.clone(), false))
        .child(view().px(px(8.0)).child(tabs(app)))
        .child(tab_body(app, id.clone()))
        .into_element();
    PhonePage {
        title: id.to_string(),
        trailing: Some(more_button(app, Record::Task(id), true)),
        body,
        bar: None,
        ask: false,
        scroll: true,
    }
}

/// Keeps what the page (or the inspector) reads loaded: the task's detail on the open list or
/// off it; for a task on the board its candidate's review detail too, refreshed as its row
/// changes; a plan's detail with every child. An empty element that holds the keeping.
pub(crate) fn keeper(app: AppState, id: Id) -> AnyElement {
    let on_board = {
        let id = id.clone();
        computed(move || app.feed.with(|feed| feed.board.task(id.as_str()).is_some()))
    };
    let plan = {
        let id = id.clone();
        computed(move || {
            app.feed.with(|feed| {
                if task_page::is_plan(feed, id.as_str()) {
                    vec![task_page::plan_ref(id.as_str())]
                } else {
                    Vec::new()
                }
            })
        })
    };
    keep::keep_details(app, plan, None);
    dynamic(move || {
        let on_board = on_board.get();
        let id = id.clone();
        untracked(move || {
            if on_board {
                candidate::keep_current(&candidate::Ctx::new(app, Some(id)));
            } else {
                let target = computed(move || {
                    app.feed
                        .with(|feed| wanted_id(feed, &id).filter(|_| feed.board.complete))
                });
                keep_loaded(app, target);
            }
            view().into_element()
        })
    })
}

/// The status mark and word, the id and the title (`page-title`), cut to fit.
fn named(app: AppState, id: &Id) -> AnyElement {
    let title = {
        let id = id.clone();
        computed(move || {
            app.feed
                .with(|feed| tasks::line(feed, id.as_str()).map(|line| (line.status, line.title)))
        })
    };
    let shown = id.to_string();
    dynamic(move || {
        let (status, words) = title.get().unzip();
        let shown = shown.clone();
        untracked(move || {
            let heading = plain(app, words.unwrap_or_default())
                .test_id("page-title")
                .role(AccessibilityRole::Header)
                .truncate()
                .flex_1()
                .min_w(px(0.0))
                .font_weight(FontWeight::BOLD);
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .children(status.map(|status| status_word_mark(app, status)))
                .child(mono(app, shown))
                .child(heading)
                .into_element()
        })
    })
}

/// `project site · priority P1 · base main · round 1 of 2`, on one line cut to fit.
fn meta(app: AppState, id: &Id) -> AnyElement {
    let line = {
        let id = id.clone();
        computed(move || {
            app.feed.with(|feed| {
                tasks::facts(feed, id.as_str())
                    .map(|facts| {
                        facts
                            .meta()
                            .iter()
                            .map(|(label, value)| format!("{label} {value}"))
                            .collect::<Vec<_>>()
                            .join(" · ")
                    })
                    .unwrap_or_default()
            })
        })
    };
    dynamic(move || {
        let line = line.get();
        (!line.is_empty()).then(|| dim(app, line).truncate().into_element())
    })
}

/// Overview, Changes and History (`tab-task-<n>`).
fn tabs(app: AppState) -> AnyElement {
    let options: Vec<(TaskTab, &'static str)> =
        TaskTab::ALL.iter().map(|tab| (*tab, tab.label())).collect();
    tab_strip(
        app,
        &options,
        move || app.nav.with(|nav| nav.task_tab),
        move |tab| app.set_task_tab(tab),
        "tab-task",
    )
}

/// The tab's body, made again when the tab changes.
fn tab_body(app: AppState, id: Id) -> AnyElement {
    let tab = computed(move || app.nav.with(|nav| nav.task_tab));
    dynamic(move || {
        let tab = tab.get();
        let id = id.clone();
        untracked(move || match tab {
            TaskTab::Overview => overview::overview(app, id),
            TaskTab::Changes => changes::changes(app, id),
            TaskTab::History => history::history(app, id),
        })
    })
}
