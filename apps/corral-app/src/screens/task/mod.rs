//! The task page: the one page of a task on every form. A plan is a task with children and a
//! review candidate a task in Review, so `Live::Plan` and `Live::Review` open it too. Its head
//! names the task; the decision band under it holds what is happening, the one next step and
//! More; then the tabs Overview, Changes and History (`NavState::task_tab`, in the address as
//! `/task/<id>/<tab>`). Escape returns to Tasks.
//!
//! This is the first cut of the page: the band's step and More, and an Overview that is the
//! task's story as the former Now inspector tells it (a Blocked task's reason, reply box and
//! next steps included). The review's decide bar and its diff move into the band and the
//! Changes tab, a plan's children into Overview, and the task's events into History.

use nsh_core::prelude::*;

use crate::model::work::{self, Record};
use crate::model::{Destination, Form, Id, TaskTab, tasks};
use crate::screens::PhonePage;
use crate::screens::now::inspector::view_of;
use crate::screens::now::parts::{keep_loaded, wanted_id};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button, tab_strip};
use crate::ui::kit::next::{more_button, next_button};
use crate::ui::kit::status::status_word_mark;
use crate::ui::kit::text::{dim, mono, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// What the Changes tab says until the review's diff moves into it.
const CHANGES_PENDING: &str =
    "The task's changes show here: the diff of its branch against its base, file by file.";
/// What the History tab says until the task's events move into it.
const HISTORY_PENDING: &str =
    "The task's history shows here: its runs, checks, comments and status changes.";

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
    let title = {
        let id = id.clone();
        computed(move || {
            app.feed
                .with(|feed| tasks::line(feed, &id).map(|line| (line.status, line.title)))
        })
    };
    let shown = id.to_string();
    let named = dynamic(move || {
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
    });
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
                    .child(named),
            )
            .child(term::hrule())
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(10.0))
            .h(px(46.0))
            .px(px(12.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(back)
            .child(named)
            .into_element(),
    };
    let pad = if form == Form::Terminal { 8.0 } else { 18.0 };
    view()
        .test_id("task-page")
        .size_full()
        .flex_grow(1.0)
        .flex_col()
        .min_h(px(0.0))
        .bg(color(Token::Bg))
        .child(head)
        .child(view().flex_none().px(px(pad)).child(band(app, &id)))
        .child(view().flex_none().px(px(pad)).child(tabs(app)))
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.0))
                .child(view().px(px(pad)).child(tab_body(app, id))),
        )
        .into_element()
}

/// The page on a phone: the shell heads it with the task's id and scrolls its body.
pub fn phone(app: AppState, id: Id) -> PhonePage {
    let body = view()
        .test_id("task-page")
        .child(view().px(px(18.0)).child(band(app, &id)))
        .child(view().px(px(8.0)).child(tabs(app)))
        .child(tab_body(app, id.clone()))
        .into_element();
    PhonePage {
        title: id.to_string(),
        body,
        trailing: None,
        bar: None,
        ask: false,
        scroll: true,
    }
}

/// The decision band: what is happening to the task, its one next step and More.
fn band(app: AppState, id: &Id) -> AnyElement {
    let form = app.form;
    let key = id.clone();
    let line = computed(move || app.feed.with(|feed| tasks::line(feed, &key)));
    let shown = id.to_string();
    let record = Record::Task(id.clone());
    dynamic(move || {
        let line = line.get();
        let (shown, record) = (shown.clone(), record.clone());
        untracked(move || {
            let now = line
                .as_ref()
                .map(|line| line.now.clone())
                .unwrap_or_default();
            let next = line.and_then(|line| line.next);
            let actions = view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .children(next.map(|next| next_button(app, &shown, &next, form == Form::Phone)))
                .child(more_button(app, record, false));
            view()
                .test_id("task-band")
                .when(form != Form::Terminal, |band| {
                    band.py(px(12.0)).gap(px(10.0))
                })
                .when(!now.is_empty(), |band| band.child(plain(app, now)))
                .child(actions)
                .into_element()
        })
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
            TaskTab::Overview => overview(app, id),
            TaskTab::Changes => pending(app, CHANGES_PENDING),
            TaskTab::History => pending(app, HISTORY_PENDING),
        })
    })
}

/// The task's story (its stages, a Blocked task's reason, reply box and next steps, its facts
/// and its body), with its detail kept loaded while the page shows it, on the open-task list
/// or off it.
fn overview(app: AppState, id: Id) -> AnyElement {
    let target = {
        let id = id.clone();
        computed(move || {
            app.feed
                .with(|feed| wanted_id(feed, &id).filter(|_| feed.board.complete))
        })
    };
    keep_loaded(app, target);
    let story = computed(move || {
        let state = app.work.get();
        app.feed.with(|feed| {
            Some(match work::task_page(&state, feed, &id) {
                work::TaskPage::Waiting(inspector) => inspector,
                work::TaskPage::Ready { story, .. } => *story,
            })
        })
    });
    view()
        .test_id("task-overview")
        .child(view_of(app, story))
        .into_element()
}

/// One quiet line where a tab's content has not moved in yet.
fn pending(app: AppState, words: &'static str) -> AnyElement {
    let line = dim(app, words);
    match app.form {
        Form::Terminal => view().child(term::blank()).child(line).into_element(),
        Form::Desktop => view()
            .py(px(16.0))
            .child(line.font_family(FONT_TEXT))
            .into_element(),
        Form::Phone => view().px(px(18.0)).py(px(16.0)).child(line).into_element(),
    }
}
