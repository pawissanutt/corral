//! Tasks: every task in scope as one list grouped by status, beside the inspector of the
//! selected one (`model::tasks`, `ui::kit::list`). A row shows the task's status, id, title,
//! what is happening, its pipeline, project, profile, age and its one next step; More holds
//! every other operation.
//!
//! This is the first cut of Tasks: the open-task list and a facts inspector. The filter
//! field, the paged Done and Dropped lists, a plan's children unfolding in place and the
//! inspector's decision band replace or extend it here.

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::list::{self, ListKey};
use crate::model::work::{Press, Record, Verb};
use crate::model::{Form, Id, Live, tasks};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::chip::chip;
use crate::ui::kit::list::{
    ListPage, Row, cell_mono, cell_text, cell_title, list_page, phone_lines,
};
use crate::ui::kit::more::press;
use crate::ui::kit::next::{more_button, next_button};
use crate::ui::kit::page::{head, search};
use crate::ui::kit::status::{pipeline_marks, pipeline_rows, status_mark, status_word_mark};
use crate::ui::kit::table::kv;
use crate::ui::kit::text::{dim, mono, plain};
use crate::ui::term;
use crate::ui::tokens::{Token, color};

/// The Tasks place on every form. The list is built again when a project comes into or leaves
/// scope, since its Project column goes and comes with it.
pub fn screen(app: AppState) -> AnyElement {
    let scoped = computed(move || app.project_scope().project().is_some());
    dynamic(move || {
        scoped.get();
        untracked(|| list(app))
    })
}

fn list(app: AppState) -> AnyElement {
    let form = app.form;
    let sections = computed(move || {
        let scope = app.project_scope();
        app.feed
            .with(|feed| app.tasks.with(|state| tasks::sections(feed, &scope, state)))
    });
    let selected = computed(move || {
        let order = sections.with(|sections| list::order(sections));
        app.nav
            .with(|nav| list::settled(&order, nav.selection.task.as_ref()))
    });
    // The first row is selected on arrival, and the palette and the keys read the selection
    // the list shows.
    effect(move || {
        let shown = selected.get();
        if shown.is_some() && untracked(|| app.selection().task) != shown {
            untracked(|| app.nav.update(|nav| nav.selection.task = shown));
        }
    });
    let notice = computed(move || {
        if !sections.with(Vec::is_empty) {
            None
        } else if app.feed.with(|feed| feed.board.complete) {
            Some(tasks::EMPTY.to_owned())
        } else {
            Some(tasks::LOADING.to_owned())
        }
    });
    let summary = computed(move || {
        let scope = app.project_scope();
        app.feed.with(|feed| tasks::summary(feed, &scope))
    });
    let page_head = dynamic(move || {
        let summary = summary.get();
        untracked(move || {
            let new = button(
                app,
                "New task",
                ButtonKind::Ghost,
                "btn-new-task",
                move || {
                    press(
                        app,
                        Press::Verb {
                            record: Record::Workspace,
                            verb: Verb::NewTask,
                        },
                    );
                },
            );
            let actions = match form {
                Form::Phone => vec![search(app), new],
                Form::Desktop | Form::Terminal => vec![
                    button(
                        app,
                        "Inspector",
                        ButtonKind::Quiet,
                        "btn-inspector",
                        move || app.toggle_inspector(),
                    ),
                    new,
                ],
            };
            head(app, tasks::TITLE, Some(summary), actions)
        })
    });
    let columns = untracked(|| tasks::columns(&app.project_scope()));
    list_page(
        app,
        ListPage {
            key: "tasks",
            head: page_head,
            columns: columns.clone(),
            sections,
            selected,
            row: Rc::new(move |id| row(app, id, &columns)),
            inspector: Some(Rc::new(move |id| inspector(app, id))),
            on_key: Rc::new(move |key| match key {
                ListKey::Select(id) => app.select(Live::Task(id)),
                ListKey::Open(id) => app.open_live(Live::Task(id)),
                ListKey::Inspector | ListKey::More(_) => {}
            }),
            on_fold: Rc::new(move |key| app.tasks.update(|state| state.toggle(key))),
            notice,
        },
    )
}

/// One task's row: a cell per column, each following the task's line.
fn row(app: AppState, id: &Id, columns: &[list::Column]) -> Row {
    let form = app.form;
    let key = id.clone();
    let line = computed(move || app.feed.with(|feed| tasks::line(feed, &key)));
    let spoken = line
        .with_untracked(|line| line.as_ref().map(tasks::TaskLine::spoken))
        .unwrap_or_default();
    let shown = id.to_string();
    let record = Record::Task(id.clone());
    let words = move |pick: fn(&tasks::TaskLine) -> String| {
        move || line.with(|line| line.as_ref().map(pick).unwrap_or_default())
    };
    let cells = if form == Form::Phone {
        let (title, now) = (
            words(|line| line.title.clone()),
            words(|line| line.now.clone()),
        );
        vec![
            dynamic(move || {
                line.with(|line| line.as_ref().map(|line| status_mark(app, line.status)))
            }),
            dynamic(move || phone_lines(app, shown.clone(), title(), now())),
            icon(Icon::Chevron, 16.0, color(Token::Ink3)),
        ]
    } else {
        columns
            .iter()
            .map(|column| cell(app, column.key, &shown, &record, line))
            .collect()
    };
    Row {
        test_id: list::row_id(&Live::Task(id.clone())),
        spoken,
        cells,
        menu: Some(record),
    }
}

/// The cell of column `key` for the task.
fn cell(
    app: AppState,
    key: &'static str,
    shown: &str,
    record: &Record,
    line: Memo<Option<tasks::TaskLine>>,
) -> AnyElement {
    let fact = move |pick: fn(&tasks::TaskLine) -> Option<String>, quiet: bool| {
        dynamic(move || {
            let words = line
                .with(|line| line.as_ref().and_then(pick))
                .unwrap_or_else(|| "–".to_owned());
            cell_text(app, words, quiet)
        })
    };
    match key {
        "status" => dynamic(move || {
            line.with(|line| line.as_ref().map(|line| status_mark(app, line.status)))
        }),
        "id" => cell_mono(app, shown.to_owned(), false),
        "title" => dynamic(move || {
            line.with(|line| {
                line.as_ref().map(|line| {
                    let tags = line
                        .priority
                        .map(str::to_owned)
                        .into_iter()
                        .chain(line.parent.as_ref().map(|parent| format!("in {parent}")))
                        .map(|tag| chip(app, tag, crate::model::Hue::Neutral))
                        .collect();
                    cell_title(app, line.title.clone(), tags)
                })
            })
        }),
        "now" => fact(|line| Some(line.now.clone()), true),
        "pipeline" => dynamic(move || {
            line.with(|line| {
                line.as_ref()
                    .map(|line| pipeline_marks(app, &line.pipeline))
            })
        }),
        "project" => fact(|line| line.project.clone(), true),
        "profile" => fact(|line| line.profile.clone(), true),
        "age" => dynamic(move || {
            let age = line.with(|line| line.as_ref().map(|line| line.age.clone()));
            cell_mono(app, age.unwrap_or_default(), true)
        }),
        _ => {
            let (shown, record) = (shown.to_owned(), record.clone());
            dynamic(move || {
                let next = line.with(|line| line.as_ref().and_then(|line| line.next.clone()));
                let (shown, record) = (shown.clone(), record.clone());
                untracked(move || {
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .children(next.map(|next| next_button(app, &shown, &next, false)))
                        .child(more_button(app, record, true))
                        .into_element()
                })
            })
        }
    }
}

/// The inspector of the selected task: its status, id and title, what is happening, its one
/// next step and More, then the pipeline spelled out and its facts.
fn inspector(app: AppState, id: &Id) -> AnyElement {
    let form = app.form;
    let key = id.clone();
    let facts = computed(move || app.feed.with(|feed| tasks::facts(feed, &key)));
    let shown = id.to_string();
    let record = Record::Task(id.clone());
    let open = {
        let id = id.clone();
        move || app.open_live(Live::Task(id.clone()))
    };
    let gap = if form == Form::Terminal { 0.0 } else { 12.0 };
    let body = dynamic(move || {
        let facts = facts.get();
        let (shown, record, open) = (shown.clone(), record.clone(), open.clone());
        untracked(move || {
            let facts = facts?;
            let line = &facts.line;
            let blank = || (form == Form::Terminal).then(term::blank);
            let actions = view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .children(
                    line.next
                        .as_ref()
                        .map(|next| next_button(app, &shown, next, false)),
                )
                .child(button(
                    app,
                    "Open page",
                    ButtonKind::Ghost,
                    "btn-open-page",
                    open,
                ))
                .child(more_button(app, record, false));
            Some(
                view()
                    .gap(px(gap))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(status_word_mark(app, line.status))
                            .child(mono(app, shown.clone())),
                    )
                    .child(plain(app, line.title.clone()).font_weight(FontWeight::BOLD))
                    .when(!line.now.is_empty(), |body| {
                        body.child(plain(app, line.now.clone()))
                    })
                    .children(facts.blocked_detail.clone().map(|detail| dim(app, detail)))
                    .children(blank())
                    .child(actions)
                    .children(blank())
                    .child(pipeline_rows(app, &line.pipeline))
                    .children(blank())
                    .child(kv(
                        app,
                        facts
                            .facts
                            .iter()
                            .map(|(label, value)| {
                                (
                                    (*label).to_owned(),
                                    plain(app, value.clone()).into_element(),
                                )
                            })
                            .collect(),
                    ))
                    .into_element(),
            )
        })
    });
    match form {
        Form::Terminal => view().px(term::cells(1.0)).child(body).into_element(),
        Form::Desktop | Form::Phone => view().p(px(16.0)).child(body).into_element(),
    }
}
