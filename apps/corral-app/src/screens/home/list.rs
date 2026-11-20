//! Home where the dashboard does not fit (a narrow terminal): the three sections as one list,
//! Needs you, Working and Just finished, tasks and sessions in one row anatomy with their one
//! next step, and Ask's composer docked under it. A row opens its page.

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::list::{self, Column, ListKey};
use crate::model::projects::Scope;
use crate::model::{Id, home};
use crate::state::AppState;
use crate::ui::kit::list::{ListPage, Row, cell_mono, cell_text, list_page};
use crate::ui::kit::next::{more_button, next_button};
use crate::ui::kit::page::head;
use crate::ui::kit::status::look_word;

use super::dash::composer;

/// The list's columns: the status word, the id, the title, what is happening, the next step.
fn columns() -> Vec<Column> {
    vec![
        Column::fixed("status", "Status", 84.0, 9.0),
        Column::fixed("id", "ID", 62.0, 8.0),
        Column::flex("title", "Title", 140.0, 16.0, 1.4),
        Column::flex("now", "Now", 110.0, 14.0, 1.0).gives_way(1),
        Column::fixed("next", "Next step", 176.0, 21.0).end(),
    ]
}

pub(super) fn screen(app: AppState, scope: Memo<Scope>) -> AnyElement {
    let sections = computed(move || {
        let scope = scope.get();
        app.feed.with(|feed| home::sections(feed, &scope))
    });
    let selected = computed(move || {
        let order = sections.with(|sections| list::order(sections));
        app.home
            .with(|state| list::settled(&order, state.selected.as_ref()))
    });
    let notice = computed(move || sections.with(Vec::is_empty).then(|| home::EMPTY.to_owned()));
    let said = computed(move || {
        let scope = scope.get();
        (
            home::title(&scope),
            app.feed.with(|feed| home::summary(feed, &scope)),
        )
    });
    let page_head = dynamic(move || {
        let (title, summary) = said.get();
        untracked(move || head(app, title, Some(summary), Vec::new()))
    })
    .into_element();
    let page = list_page(
        app,
        ListPage {
            key: "home",
            head: page_head,
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
    );
    view()
        .size_full()
        .flex_grow(1.0)
        .flex_col()
        .min_h(px(0.0))
        .child(view().flex_1().min_h(px(0.0)).child(page))
        .child(composer(app))
        .into_element()
}

/// One row: a task or a session in the same anatomy. Its words follow the feed.
fn row(app: AppState, id: &Id) -> Row {
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
    Row {
        test_id,
        spoken,
        cells: vec![
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
        ],
        menu: Some(record),
    }
}
