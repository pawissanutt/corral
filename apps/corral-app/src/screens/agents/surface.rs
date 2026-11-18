//! The session's terminal surface: the toolbar with the read state and Refresh, the rows inside
//! their scroll (a pane's screen row for row, or the classed lines of a process log), and the
//! terminal view and phone terminal built from them. The session page and the roster's panel
//! share it.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{Action, Feed, Line, Session, Terminal, screen};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::console::{self, BIG, PHONE, PHONE_SCREEN, SCREEN};
use super::{Dispatch, grid, keys, run};

/// What the terminal surface draws below its toolbar; a new screen redraws only this.
#[derive(Clone, PartialEq)]
pub(super) struct Rows {
    lines: Vec<Line>,
    empty: Option<String>,
    cursor: bool,
    verbatim: bool,
    styled: Vec<screen::Row>,
}

impl Rows {
    pub(super) fn of(screen: &Terminal) -> Self {
        Self {
            lines: screen.lines.clone(),
            empty: screen.empty.clone(),
            cursor: screen.cursor,
            verbatim: screen.verbatim,
            styled: screen.styled.clone(),
        }
    }
}

/// The toolbar's words and control, apart from the rows: it is redrawn only when the read state
/// changes, so its Refresh keeps the focus while the screen updates.
#[derive(Clone, PartialEq)]
pub(super) struct Termbar {
    feed: Feed,
    heading: String,
    status: String,
    input: String,
    refresh: Option<Action>,
}

impl Termbar {
    pub(super) fn of(screen: &Terminal) -> Self {
        Self {
            feed: screen.feed,
            heading: screen.heading.clone(),
            status: screen.status.clone(),
            input: screen.input.clone(),
            refresh: screen.refresh.clone(),
        }
    }
}

fn rows_of(session: Memo<Session>) -> Memo<Option<Rows>> {
    computed(move || session.with(|s| s.terminal.as_ref().map(Rows::of)))
}

fn termbar_of(session: Memo<Session>) -> Memo<Option<Termbar>> {
    computed(move || session.with(|s| s.terminal.as_ref().map(Termbar::of)))
}

/// What the terminal surface shows inside its scroll, `sides` and `ends` in from its edges (a
/// Cell aside in a real terminal): a pane's screen row for row, the classed lines, or why there
/// are none.
pub(super) fn rows_view(
    app: AppState,
    rows: Option<&Rows>,
    set: console::Set,
    (sides, ends): (f32, f32),
) -> AnyElement {
    let Some(rows) = rows else {
        return view().into_element();
    };
    let empty = rows.empty.clone().filter(|_| rows.lines.is_empty());
    if app.form == Form::Terminal {
        return match empty {
            Some(words) => view()
                .px(term::cells(1.0))
                .child(term::quiet(words))
                .into_element(),
            None if rows.verbatim && !rows.styled.is_empty() => grid::cell_screen(&rows.styled),
            None if rows.verbatim => console::cell_screen(&rows.lines),
            None => view()
                .px(term::cells(1.0))
                .child(console::cell_lines(app, &rows.lines))
                .into_element(),
        };
    }
    let padded = |content: AnyElement| {
        view()
            .px(px(sides))
            .py(px(ends))
            .child(content)
            .into_element()
    };
    match empty {
        Some(words) => padded(console::empty(words, set)),
        None if rows.verbatim => {
            let screen = if app.form == Form::Phone {
                PHONE_SCREEN
            } else {
                SCREEN
            };
            if rows.styled.is_empty() {
                console::screen_rows(&rows.lines, screen, (sides, ends))
            } else {
                grid::screen(&rows.styled, screen, (sides, ends))
            }
        }
        None => padded(console::lines(&rows.lines, set, rows.cursor)),
    }
}

/// The pressable Refresh for a toolbar, when the terminal can be revalidated.
fn refresh_control(app: AppState, bar: &Termbar, send: Dispatch) -> Option<AnyElement> {
    bar.refresh
        .clone()
        .map(|action| console::refresh(move || send(app, action.clone())))
}

/// The toolbar over the terminal surface for the form, its Refresh sent through `send`.
pub(super) fn toolbar(app: AppState, bar: &Termbar, send: Dispatch) -> AnyElement {
    if app.form == Form::Terminal {
        return cell_toolbar(app, bar, send);
    }
    console::toolbar(
        bar.feed,
        &bar.heading,
        &bar.status,
        &bar.input,
        refresh_control(app, bar, send),
    )
}

/// The real terminal's toolbar: its words quiet on one row, the read state held once the rows
/// are not current, and the refresh control.
fn cell_toolbar(app: AppState, bar: &Termbar, send: Dispatch) -> AnyElement {
    view()
        .flex_row()
        .h(term::rows(1.0))
        .px(term::cells(1.0))
        .child(term::quiet(bar.heading.clone()))
        .child(term::space(2.0))
        .child(
            term::quiet(bar.status.clone())
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .text_color(color(match bar.feed {
                    Feed::Live => Token::Ink3,
                    Feed::Snapshot => Token::Held,
                })),
        )
        .child(view().flex_1().min_w(px(0.0)))
        .child(
            term::quiet(bar.input.clone())
                .flex_shrink(1.0)
                .min_w(px(0.0)),
        )
        .when_some(bar.refresh.clone(), |row, action| {
            row.child(term::space(2.0)).child(button_with(
                app,
                "Refresh",
                ButtonKind::Ghost,
                ButtonOptions::default(),
                "btn-refresh-capture",
                move || send(app, action.clone()),
            ))
        })
        .into_element()
}

/// The terminal surface's scroll: it opens at its newest rows and keeps to them as the screen
/// changes, unless the owner scrolled up.
pub(super) fn surface_scroll(rows: Memo<Option<Rows>>) -> Scroll {
    console::follow_end(
        scroll().test_id("session-terminal").flex_1().min_h(px(0.0)),
        move || rows.with(|_| ()),
    )
}

/// The terminal view. Desktop: the toolbar over the dark mono surface, scrolling, and the
/// pane's keys bar and type field under it. Terminal: the toolbar's words quiet on one row with
/// the refresh control, a rule, the lines, then the keys.
pub(super) fn terminal_view(app: AppState, session: Memo<Session>) -> AnyElement {
    let bar = termbar_of(session);
    let rows = rows_of(session);
    let keys = keys::region(app, session);
    let top = dynamic(move || {
        bar.with(|bar| match bar {
            Some(bar) => toolbar(app, bar, run),
            None => view().into_element(),
        })
    });
    if app.form == Form::Terminal {
        return view()
            .flex_1()
            .min_h(px(0.0))
            .child(top)
            .child(term::hrule())
            .child(surface_scroll(rows).child(dynamic(move || {
                rows.with(|rows| rows_view(app, rows.as_ref(), BIG, (0.0, 0.0)))
            })))
            .child(keys)
            .into_element();
    }
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(top)
        .child(
            surface_scroll(rows)
                .bg(console::ground())
                .child(dynamic(move || {
                    rows.with(|rows| rows_view(app, rows.as_ref(), BIG, (18.0, 14.0)))
                })),
        )
        .child(keys)
        .into_element()
}

/// The terminal on the phone: the read state and Refresh over the dark surface,
/// the screen at its full width, panning sideways.
pub(super) fn phone_terminal(app: AppState, session: Memo<Session>) -> AnyElement {
    let bar = termbar_of(session);
    let rows = rows_of(session);
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(dynamic(move || {
            bar.with(|bar| match bar {
                Some(bar) => {
                    console::phone_toolbar(bar.feed, &bar.status, refresh_control(app, bar, run))
                }
                None => view().into_element(),
            })
        }))
        .child(
            surface_scroll(rows)
                .bg(console::ground())
                .child(dynamic(move || {
                    rows.with(|rows| rows_view(app, rows.as_ref(), PHONE, (12.0, 12.0)))
                })),
        )
        .into_element()
}
