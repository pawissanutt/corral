//! The inspector beside Tasks (`.insp`): the selected task's status, id, Open page and More, its
//! title; then the same decision band as its page, so a decision is made without leaving the
//! list; then its facts (round, priority, profile and base among them), its pipeline spelled
//! out, a plan's children, the files its candidate changed, its description, its runs and its
//! last events.

use nsh_core::prelude::*;

use crate::model::tasks::{self, page};
use crate::model::work::Record;
use crate::model::{Form, Id, Live, TaskTab};
use crate::screens::markdown::{markdown, provide_styles};
use crate::screens::task::parts::{block, event, link_row, linked, run};
use crate::screens::task::{decision, keeper};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::next::more_button;
use crate::ui::kit::status::{pipeline_rows, status_word_mark};
use crate::ui::kit::table::kv;
use crate::ui::kit::text::{mono, plain};
use crate::ui::term;

/// How many of the task's events the inspector shows; the page's History has them all.
const EVENTS: usize = 5;

/// The inspector of the row `key` (a row under a plan inspects its child).
pub(super) fn inspector(app: AppState, key: &Id) -> AnyElement {
    let form = app.form;
    let id = tasks::task_of(key);
    let facts = {
        let id = id.clone();
        computed(move || app.feed.with(|feed| tasks::facts(feed, id.as_str())))
    };
    let body = view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .child(keeper(app, id.clone()))
        .child(header(app, &id, facts))
        .child(decision(app, id.clone(), true))
        .child(sections(app, id, facts));
    match form {
        Form::Terminal => view().px(term::cells(1.0)).child(body).into_element(),
        Form::Desktop | Form::Phone => view().px(px(16.0)).py(px(14.0)).child(body).into_element(),
    }
}

/// The status, id, Open page (`btn-open-page`) and More on one line, the title under them.
fn header(app: AppState, id: &Id, facts: Memo<Option<tasks::TaskFacts>>) -> AnyElement {
    let shown = id.to_string();
    let line = computed(move || {
        facts.with(|facts| {
            facts
                .as_ref()
                .map(|facts| (facts.line.status, facts.line.title.clone()))
        })
    });
    let id = id.clone();
    dynamic(move || {
        let (status, title) = line.get()?;
        let (shown, id) = (shown.clone(), id.clone());
        Some(untracked(move || {
            let live = Live::Task(id.clone());
            let open = button(
                app,
                page::OPEN_PAGE,
                ButtonKind::Ghost,
                "btn-open-page",
                move || app.open_live(live.clone()),
            );
            view()
                .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
                .pb(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.0))
                        .child(status_word_mark(app, status))
                        .child(mono(app, shown))
                        .child(view().flex_1())
                        .child(open)
                        .child(more_button(app, Record::Task(id), true)),
                )
                .child(plain(app, title).font_weight(FontWeight::BOLD))
                .into_element()
        }))
    })
}

/// Everything under the band, each block following its own facts.
fn sections(app: AppState, id: Id, facts: Memo<Option<tasks::TaskFacts>>) -> AnyElement {
    let fact_rows = dynamic(move || {
        facts.with(|facts| {
            let facts = facts.as_ref()?;
            let rows = facts
                .facts
                .iter()
                .map(|(label, value)| {
                    (
                        (*label).to_owned(),
                        plain(app, value.clone()).into_element(),
                    )
                })
                .collect();
            Some(block(app, page::FACTS, None, kv(app, rows)))
        })
    });
    let pipeline = dynamic(move || {
        facts.with(|facts| {
            let facts = facts.as_ref()?;
            Some(block(
                app,
                page::PIPELINE,
                None,
                view()
                    .test_id("pipeline")
                    .child(pipeline_rows(app, &facts.line.pipeline))
                    .into_element(),
            ))
        })
    });
    let body = computed(move || facts.with(|facts| facts.as_ref().and_then(|f| f.body.clone())));
    provide_styles(app);
    let description = dynamic(move || {
        body.get()
            .map(|body| block(app, page::DESCRIPTION, None, markdown(app, &body, false)))
    });
    view()
        .child(fact_rows)
        .child(pipeline)
        .child(children(app, id.clone()))
        .child(files(app, id.clone()))
        .child(description)
        .child(runs(app, id.clone()))
        .child(history(app, id))
        .into_element()
}

/// A plan's children, each opening its page.
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

/// The files the candidate changed, each opening the page's Changes tab.
fn files(app: AppState, id: Id) -> AnyElement {
    let list = {
        let id = id.clone();
        computed(move || app.feed.with(|feed| page::files(feed, id.as_str())))
    };
    dynamic(move || {
        let list = list.get();
        let id = id.clone();
        (!list.is_empty()).then(|| {
            let rows: Vec<AnyElement> = list
                .iter()
                .enumerate()
                .map(|(at, file)| {
                    let live = Live::Task(id.clone());
                    link_row(
                        app,
                        format!("change-file-{at}"),
                        format!("{} {}", file.path, file.counts),
                        Vec::new(),
                        file.path.clone(),
                        file.counts.clone(),
                        move || {
                            app.open_live(live.clone());
                            app.set_task_tab(TaskTab::Changes);
                        },
                    )
                })
                .collect();
            block(
                app,
                page::CHANGES,
                Some(page::file_count(list.len())),
                view().children(rows).into_element(),
            )
        })
    })
}

/// The task's runs, each opening its session.
fn runs(app: AppState, id: Id) -> AnyElement {
    let list = computed(move || app.feed.with(|feed| page::runs(feed, id.as_str())));
    dynamic(move || {
        let list = list.get();
        (!list.is_empty()).then(|| {
            let rows: Vec<AnyElement> = list.iter().map(|line| run(app, line)).collect();
            block(app, page::RUNS, None, view().children(rows).into_element())
        })
    })
}

/// The task's last events, newest first.
fn history(app: AppState, id: Id) -> AnyElement {
    let list = computed(move || {
        app.feed.with(|feed| {
            let mut events = page::history(feed, id.as_str());
            events.truncate(EVENTS);
            events
        })
    });
    dynamic(move || {
        let list = list.get();
        (!list.is_empty()).then(|| {
            let rows: Vec<AnyElement> = list.iter().map(|line| event(app, line)).collect();
            block(
                app,
                page::HISTORY,
                None,
                view().children(rows).into_element(),
            )
        })
    })
}
