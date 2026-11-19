//! The session's sidebar (`session::Sidebar`): Context, Files and Plan. Beside the main view
//! when the page is wide (desktop 280 pt from 960 pt of content, terminal 34 Cells from 105) and
//! the owner has not hidden it with Alt+B: its tabs, the shown tab's summary and where its read
//! stands, and ▸ to hide it. Otherwise one summary row above the composer (`Context 2 running ·
//! 1 waiting   Files 12   Plan 3/7   ▴`, a spinner before it while anything works), whose labels
//! open a drawer under it with the same tabs and body. The phone always has the row.
//! test ids `side-<tab>`, `side-row-<tab>`, `btn-sidebar-hide`, `btn-sidebar-drawer`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::session::{SessionPage, SideTabItem, Sidebar};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::mark;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::super::run;
use super::hue_words;
use super::tabs::part;

/// The desktop rail's width, and the terminal's in Cells.
const RAIL: f32 = 280.0;
const RAIL_CELLS: f32 = 34.0;

fn sidebar_of(page: Memo<SessionPage>) -> Memo<Option<Sidebar>> {
    computed(move || page.with(|p| p.sidebar.clone()))
}

/// The rail beside the main view.
pub(super) fn rail(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let form = app.form;
    let sidebar = sidebar_of(page);
    let toggle = computed(move || sidebar.with(|s| s.as_ref().map(|s| s.toggle.clone())));
    let hide = dynamic(move || {
        toggle.get().map(|action| {
            untracked(|| {
                button_with(
                    app,
                    "▸",
                    ButtonKind::Quiet,
                    ButtonOptions {
                        key_hint: Some("Alt+B"),
                        ..ButtonOptions::default()
                    },
                    "btn-sidebar-hide",
                    move || run(app, action.clone()),
                )
            })
        })
    });
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 8.0 } else { 4.0 }))
        .child(tab_row(app, sidebar))
        .child(view().flex_1().min_w(px(0.0)))
        .child(hide);
    let body = scroll()
        .test_id("session-sidebar")
        .flex_1()
        .min_h(px(0.0))
        .child(body(app, sidebar));
    if form == Form::Terminal {
        return view()
            .flex_row()
            .flex_none()
            .child(term::vrule())
            .child(
                view()
                    .w(term::cells(RAIL_CELLS))
                    .flex_col()
                    .min_h(px(0.0))
                    .px(term::cells(1.0))
                    .child(head)
                    .child(body),
            )
            .into_element();
    }
    view()
        .w(px(RAIL))
        .flex_none()
        .flex_col()
        .min_h(px(0.0))
        .border_l(px(1.0))
        .border_l_color(color(Token::Rule))
        .child(
            head.h(px(38.0))
                .px(px(10.0))
                .border_b(px(1.0))
                .border_color(color(Token::Rule)),
        )
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .px(px(12.0))
                .py(px(10.0))
                .child(body),
        )
        .into_element()
}

/// The shown tab's body: its summary and where its read stands.
fn body(app: AppState, sidebar: Memo<Option<Sidebar>>) -> AnyElement {
    let shown = computed(move || sidebar.with(|s| s.as_ref().map(|s| s.body.clone())));
    dynamic(move || {
        shown
            .get()
            .map(|body| untracked(|| part(app, &body, "side")))
    })
}

/// The tabs the host serves, the shown one marked; pressing one shows it, and the pick stays.
fn tab_row(app: AppState, sidebar: Memo<Option<Sidebar>>) -> AnyElement {
    let tabs =
        computed(move || sidebar.with(|s| s.as_ref().map(|s| s.tabs.clone()).unwrap_or_default()));
    dynamic(move || {
        let tabs = tabs.get();
        untracked(|| {
            view()
                .flex_row()
                .items_center()
                .gap(px(if app.form == Form::Terminal { 8.0 } else { 2.0 }))
                .children(
                    tabs.iter()
                        .map(|tab| side_tab(app, tab))
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
    })
}

fn side_tab(app: AppState, item: &SideTabItem) -> AnyElement {
    let form = app.form;
    let label = text(item.label)
        .number_of_lines(1u32)
        .text_color(color(if item.selected {
            Token::Ink
        } else {
            Token::Ink3
        }))
        .font_weight(if item.selected {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        });
    let action = item.action.clone();
    let tab = view()
        .test_id(&item.id)
        .role(AccessibilityRole::Button)
        .label(item.label)
        .accessibility_state(AccessibilityState {
            selected: item.selected,
            ..AccessibilityState::default()
        })
        .flex_row()
        .items_center()
        .cursor_pointer()
        .on_press(move || run(app, action.clone()))
        .when(item.selected, |tab| tab.bg(color(Token::Sel)));
    match form {
        Form::Terminal => tab.px(term::cells(1.0)).child(label).into_element(),
        Form::Desktop | Form::Phone => tab
            .h(px(if form == Form::Phone { 36.0 } else { 26.0 }))
            .px(px(8.0))
            .rounded(px(5.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .child(label.font_family(FONT_TEXT).text_size(px(12.5)))
            .into_element(),
    }
}

/// The summary row standing for the sidebar, and its drawer while open.
pub(super) fn summary(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let form = app.form;
    let open = signal(false);
    let sidebar = sidebar_of(page);
    let summary = computed(move || sidebar.with(|s| s.as_ref().map(|s| s.summary.clone())));
    let row = dynamic(move || {
        let summary = summary.get()?;
        Some(untracked(|| {
            let segments: Vec<AnyElement> = summary
                .segments
                .into_iter()
                .map(|segment| {
                    let action = segment.action;
                    let said = text(segment.label.clone())
                        .number_of_lines(1u32)
                        .text_color(hue_words(form, segment.hue));
                    let said = match form {
                        Form::Terminal => said,
                        _ => said.font_family(FONT_TEXT).text_size(px(12.0)),
                    };
                    view()
                        .test_id(&format!("side-row-{}", segment.tab.slug()))
                        .role(AccessibilityRole::Button)
                        .label(segment.label)
                        .cursor_pointer()
                        .on_press(move || {
                            run(app, action.clone());
                            open.set(true);
                        })
                        .flex_shrink_0()
                        .child(said)
                        .into_element()
                })
                .collect();
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(if form == Form::Terminal { 24.0 } else { 16.0 }))
                .children(
                    summary
                        .working
                        .then(|| mark(app, Mark::Active, Hue::Worker)),
                )
                .children(segments)
                .into_element()
        }))
    });
    let caret = dynamic(move || {
        let label = if open.get() { "▾" } else { "▴" };
        untracked(|| {
            button_with(
                app,
                label,
                ButtonKind::Quiet,
                ButtonOptions::default(),
                "btn-sidebar-drawer",
                move || open.update(|open| *open = !*open),
            )
        })
    });
    let line = view()
        .test_id("session-sidebar-row")
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(view().flex_1().min_w(px(0.0)).child(row))
        .child(caret);
    let drawer = dynamic(move || {
        open.get().then(|| {
            untracked(|| {
                let content = scroll()
                    .test_id("session-sidebar")
                    .max_h(if form == Form::Terminal {
                        term::rows(8.0)
                    } else {
                        px(220.0)
                    })
                    .child(body(app, sidebar));
                view()
                    .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
                    .child(tab_row(app, sidebar))
                    .child(content)
                    .into_element()
            })
        })
    });
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(line)
        .child(drawer)
        .into_element()
}
