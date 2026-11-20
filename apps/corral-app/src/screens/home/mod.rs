//! Home: what waits on the owner, then what is working, tasks and sessions in one list
//! (`model::home`). A row opens its page; its one next step is on the row and every other
//! operation under More. In a workspace with no projects Home is the assistant's landing
//! (`screens::landing`, which the router shows in its place).
//!
//! This is the first cut of Home: the two sections as one list on every form. The dashboard a
//! wide screen gets (tiles, the open card, the panels and the docked composer) and Just
//! finished replace it here.

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::list::{self, Column, ListKey};
use crate::model::{Form, Id, home};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::list::{ListPage, Row, cell_mono, cell_text, list_page, phone_lines};
use crate::ui::kit::next::{more_button, next_button};
use crate::ui::kit::page::{head, search};
use crate::ui::kit::status::{look_mark, look_word};
use crate::ui::tokens::{Token, color};

/// Home's columns: the status word, the id, the title, what is happening and the next step.
fn columns() -> Vec<Column> {
    vec![
        Column::fixed("status", "Status", 84.0, 9.0),
        Column::fixed("id", "ID", 62.0, 8.0),
        Column::flex("title", "Title", 140.0, 16.0, 1.4),
        Column::flex("now", "Now", 110.0, 14.0, 1.0).gives_way(1),
        Column::fixed("next", "Next step", 176.0, 21.0).end(),
    ]
}

pub fn screen(app: AppState) -> AnyElement {
    let form = app.form;
    let sections = computed(move || {
        let scope = app.project_scope();
        app.feed.with(|feed| home::sections(feed, &scope))
    });
    let selected = computed(move || {
        let order = sections.with(|sections| list::order(sections));
        app.home
            .with(|state| list::settled(&order, state.selected.as_ref()))
    });
    let notice = computed(move || sections.with(Vec::is_empty).then(|| home::EMPTY.to_owned()));
    // A phone's tab root keeps the palette's search in its head.
    let actions = if form == Form::Phone {
        vec![search(app)]
    } else {
        Vec::new()
    };
    list_page(
        app,
        ListPage {
            key: "home",
            head: head(app, home::TITLE, None, actions),
            columns: columns(),
            sections,
            selected,
            row: Rc::new(move |id| row(app, id)),
            inspector: None,
            on_key: Rc::new(move |key| match key {
                ListKey::Select(id) => app.home.update(|state| state.selected = Some(id)),
                ListKey::Open(id) => {
                    let live = app.feed.with_untracked(|feed| home::live_of(feed, &id));
                    app.open_live(live);
                }
                ListKey::Inspector | ListKey::More(_) => {}
            }),
            on_fold: Rc::new(|_| {}),
            notice,
        },
    )
}

/// One row: a task or a session in the same anatomy. Its words follow the feed.
fn row(app: AppState, id: &Id) -> Row {
    let form = app.form;
    let key = id.clone();
    let line = computed(move || app.feed.with(|feed| home::row(feed, &key)));
    let (test_id, spoken, record) = app.feed.with_untracked(|feed| {
        (
            list::row_id(&home::live_of(feed, id)),
            home::row(feed, id)
                .map(|row| row.spoken())
                .unwrap_or_default(),
            home::record_of(feed, id),
        )
    });
    let shown = id.to_string();
    let title = move || line.with(|line| line.as_ref().map(|line| line.title.clone()));
    let now = move || line.with(|line| line.as_ref().map(|line| line.now.clone()));
    let step = {
        let (shown, record) = (shown.clone(), record.clone());
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
    };
    let cells = if form == Form::Phone {
        vec![
            dynamic(move || {
                line.with(|line| {
                    line.as_ref()
                        .map(|line| look_mark(app, line.look, line.word))
                })
            }),
            dynamic(move || {
                phone_lines(
                    app,
                    shown.clone(),
                    title().unwrap_or_default(),
                    now().unwrap_or_default(),
                )
            }),
            icon(Icon::Chevron, 16.0, color(Token::Ink3)),
        ]
    } else {
        vec![
            dynamic(move || {
                line.with(|line| {
                    line.as_ref()
                        .map(|line| look_word(app, line.look, line.word))
                })
            }),
            cell_mono(app, shown, false),
            dynamic(move || cell_text(app, title().unwrap_or_default(), false)),
            dynamic(move || cell_text(app, now().unwrap_or_default(), true)),
            step,
        ]
    };
    Row {
        test_id,
        spoken,
        cells,
        menu: Some(record),
    }
}
