//! The session's sidebar (`session::Sidebar`): Context, Files and Plan. Beside the main view
//! when the page is wide (desktop 280 pt from 960 pt of content, terminal 34 Cells from 105) and
//! the owner has not hidden it with Alt+B: its tabs, the shown tab's summary and where its read
//! stands, its rows, and ▸ to hide it. Otherwise it is never gone: one summary row above the
//! composer (`Context 2 running · 1 waiting   Files 12   Plan 3/7   ▴`, a spinner before it
//! while anything works), whose labels open a drawer under it with the same tabs, body and rows
//! (desktop and terminal) or a bottom sheet (phone). The Plan ledger and Host events sheets are
//! mounted with the page (`sheets`). test ids `side-<tab>`, `side-row-<tab>`,
//! `btn-sidebar-hide`, `btn-sidebar-drawer`, `session-sidebar`, `session-sidebar-sheet`.

mod context;
mod files;

use nsh_core::prelude::*;

use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::session::{Cx, Facts, SessionPage, SideContent, SideTabItem, Sidebar};
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::mark;
use crate::ui::kit::overlay::{self, Modal};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::super::run;
use super::tabs::part;
use super::{events, hue_words, plan};

/// The desktop rail's width, and the terminal's in Cells.
const RAIL: f32 = 280.0;
const RAIL_CELLS: f32 = 34.0;
/// The drawer's greatest height under the summary row: points on desktop, rows in a terminal.
const DRAWER: f32 = 320.0;
const DRAWER_ROWS: f32 = 10.0;

fn sidebar_of(page: Memo<SessionPage>) -> Memo<Option<Sidebar>> {
    computed(move || page.with(|p| p.sidebar.clone()))
}

/// Words and captions set in from the edge the rows run to, as the form spaces them.
pub(super) fn inset(app: AppState, child: impl IntoElement) -> AnyElement {
    let side = view().min_w(px(0.0));
    match app.form {
        Form::Terminal => side.px(term::cells(1.0)),
        Form::Desktop => side.px(px(12.0)),
        Form::Phone => side.px(px(18.0)),
    }
    .child(child)
    .into_element()
}

/// The Plan ledger and Host events sheets of session `id`, mounted once with its page.
pub(super) fn sheets(app: AppState, id: Id) -> AnyElement {
    view()
        .child(plan::sheet(app, id.clone()))
        .child(events::sheet(app, id))
        .into_element()
}

/// What `read` makes of session `id` as the model reads it now (tracked): for the sheets, which
/// open over the page whether or not the sidebar shows.
pub(super) fn with_cx<T>(app: AppState, id: &Id, read: impl Fn(&Cx<'_>) -> T) -> T {
    let env = app.env();
    app.feed.with(|feed| {
        app.session.with(|state| {
            let facts = Facts::of(feed, id);
            let cx = Cx::new(feed, state.ui(id.as_str()), id, env, &facts);
            read(&cx)
        })
    })
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
        .child(shown(app, sidebar));
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
                    .child(head.px(term::cells(1.0)))
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
        .child(view().flex_1().min_h(px(0.0)).py(px(10.0)).child(body))
        .into_element()
}

/// The shown tab: its summary and where its read stands, then its rows.
fn shown(app: AppState, sidebar: Memo<Option<Sidebar>>) -> AnyElement {
    let frame = computed(move || sidebar.with(|s| s.as_ref().map(|s| s.body.clone())));
    let rows = computed(move || sidebar.with(|s| s.as_ref().map(|s| s.content.clone())));
    let gap = if app.form == Form::Terminal { 0.0 } else { 6.0 };
    view()
        .gap(px(gap))
        .child(dynamic(move || {
            frame
                .get()
                .map(|body| untracked(|| inset(app, part(app, &body, "side"))))
        }))
        .child(dynamic(move || {
            rows.get().map(|content| {
                untracked(|| match content {
                    SideContent::Context(tab) => context::context(app, &tab),
                    SideContent::Files(tab) => files::files(app, &tab),
                    SideContent::Plan(tab) => plan::tab(app, &tab),
                })
            })
        }))
        .into_element()
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
            .px(px(if form == Form::Phone { 12.0 } else { 8.0 }))
            .rounded(px(5.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .child(
                label
                    .font_family(FONT_TEXT)
                    .text_size(px(if form == Form::Phone { 14.0 } else { 12.5 })),
            )
            .into_element(),
    }
}

/// The summary row standing for the sidebar, and what it opens: the drawer under it (desktop,
/// terminal) or the bottom sheet (phone).
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
                        .when(form == Form::Phone, |segment| {
                            segment.min_h(px(36.0)).justify_center()
                        })
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
    if form == Form::Phone {
        return view()
            .child(line)
            .child(overlay::modal(
                app,
                move || open.get(),
                move || open.set(false),
                move || phone_sheet(app, sidebar),
            ))
            .into_element();
    }
    let drawer = dynamic(move || {
        open.get().then(|| {
            untracked(|| {
                let content = scroll()
                    .test_id("session-sidebar")
                    .max_h(if form == Form::Terminal {
                        term::rows(DRAWER_ROWS)
                    } else {
                        px(DRAWER)
                    })
                    .child(shown(app, sidebar));
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

/// The phone's sheet: the three tabs over the shown tab's body and rows.
fn phone_sheet(app: AppState, sidebar: Memo<Option<Sidebar>>) -> Modal {
    Modal {
        test_id: "session-sidebar-sheet".into(),
        title: "Sidebar".into(),
        sub: None,
        body: view()
            .gap(px(10.0))
            .child(inset(app, tab_row(app, sidebar)))
            .child(view().test_id("session-sidebar").child(shown(app, sidebar)))
            .into_element(),
        foot: None,
        done: Some("Done".into()),
        confirm: None,
    }
}
