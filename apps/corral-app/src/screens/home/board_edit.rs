//! Customize on Home's board: the tray of widgets to add, and over each widget its handles.
//! A widget's head moves it (drag it onto another widget: the accent bar shows the side it
//! lands on), its right edge resizes it a column at a time, and its buttons do the same one
//! step per press, so the keyboard and screen readers arrange the board too.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::home::board::COLUMNS;
use crate::model::home::board::{
    self, Drop, Frame, Item, Widget, dragged_span, drop_at, shown_span,
};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::board::{Cells, GAP};
use super::parts::{surface, words};

/// The widget being moved and where it would drop now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Moving {
    pub widget: Widget,
    pub drop: Option<Drop>,
}

/// Over the board while Customize is on: the widgets not on it, each added by a press, what
/// the handles do, and Reset layout.
pub(super) fn tray(app: AppState) -> AnyElement {
    let form = app.form;
    let hidden = computed(move || app.board.board.with(board::Board::hidden));
    let chips = dynamic(move || {
        let hidden = hidden.get();
        untracked(move || {
            if hidden.is_empty() {
                return words(form, board::ALL_SHOWN, 12.0, Token::Ink3).into_element();
            }
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(6.0))
                .min_w(px(0.0))
                .children(
                    hidden
                        .into_iter()
                        .map(|widget| {
                            button(
                                app,
                                format!("+ {}", widget.title()),
                                ButtonKind::Ghost,
                                format!("btn-board-add-{}", widget.slug()),
                                move || app.board.change(|b| b.add(widget)),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
    });
    surface("board-tray")
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(10.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .min_w(px(0.0))
                .child(
                    text(board::ADD)
                        .role(AccessibilityRole::Header)
                        .number_of_lines(1u32)
                        .font_family(FONT_TEXT)
                        .text_size(px(12.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink)),
                )
                .child(view().flex_1().min_w(px(0.0)).child(words(
                    form,
                    board::EDITING_NOTE,
                    12.0,
                    Token::Ink3,
                )))
                .child(button(
                    app,
                    board::RESET,
                    ButtonKind::Quiet,
                    "btn-board-reset",
                    move || app.board.reset(),
                )),
        )
        .child(chips)
        .into_element()
}

/// The accent bar on the side of `widget` where the moved widget would land.
pub(super) fn drop_mark(widget: Widget, moving: Signal<Option<Moving>>) -> AnyElement {
    let side = computed(move || {
        moving.with(|m| {
            m.and_then(|m| m.drop)
                .filter(|d| d.target == widget)
                .map(|d| d.after)
        })
    });
    dynamic(move || {
        let after = side.get()?;
        let bar = view()
            .absolute()
            .top(px(GAP / 2.0))
            .bottom(px(GAP / 2.0))
            .w(px(4.0))
            .rounded(px(2.0))
            .bg(color(Token::Focus));
        Some(if after {
            bar.right(px(1.0))
        } else {
            bar.left(px(1.0))
        })
    })
    .into_element()
}

/// Over one widget while Customize is on: a veil that keeps presses off the widget, its head
/// to drag it by with its buttons, and its right edge to resize it by.
pub(super) fn handles(
    app: AppState,
    widget: Widget,
    item: ReadSignal<Item>,
    width: Signal<f32>,
    moving: Signal<Option<Moving>>,
    cells: Cells,
) -> AnyElement {
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .h(px(30.0))
        .pl(px(4.0))
        .pr(px(4.0))
        .bg(color(Token::Surface2))
        .border_b_1()
        .border_color(color(Token::Rule2))
        .child(grip(app, widget, moving, cells))
        .child(tool("←", "Move earlier", "earlier", widget, move || {
            app.board.change(|b| b.step(widget, -1));
        }))
        .child(tool("→", "Move later", "later", widget, move || {
            app.board.change(|b| b.step(widget, 1));
        }))
        .child(tool("−", "Narrower", "narrower", widget, move || {
            app.board.change(|b| b.nudge(widget, false));
        }))
        .child(tool("+", "Wider", "wider", widget, move || {
            app.board.change(|b| b.nudge(widget, true));
        }))
        .child(tool("×", "Remove", "remove", widget, move || {
            app.board.change(|b| b.remove(widget));
        }));
    view()
        .test_id(&format!("board-edit-{}", widget.slug()))
        .absolute()
        .top(px(GAP / 2.0))
        .left(px(GAP / 2.0))
        .right(px(GAP / 2.0))
        .bottom(px(GAP / 2.0))
        .rounded(px(8.0))
        .overflow_hidden()
        .border_2()
        .border_dashed()
        .border_color(color(Token::Focus))
        .bg(color(Token::Bg).opacity(0.55))
        .child(head)
        .child(edge(app, widget, item, width))
        .into_element()
}

/// The head's left part: the widget's name, dragged to move it.
fn grip(app: AppState, widget: Widget, moving: Signal<Option<Moving>>, cells: Cells) -> AnyElement {
    let frames: Rc<RefCell<Vec<(Widget, Frame)>>> = Rc::default();
    let measured = frames.clone();
    let moved = frames.clone();
    let dropped = frames;
    let follow = move |x: f32, y: f32| {
        let drop = drop_at(&moved.borrow(), widget, x, y);
        let next = Some(Moving { widget, drop });
        if moving.get_untracked() != next {
            moving.set(next);
        }
    };
    view()
        .test_id(&format!("board-grip-{}", widget.slug()))
        .flex_1()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .h(relative(1.0))
        .min_w(px(0.0))
        .px(px(4.0))
        .cursor(Cursor::Grab)
        .label(format!("{}: drag to move", widget.title()))
        .child(
            text("⠿")
                .flex_shrink_0()
                .text_size(px(14.0))
                .text_color(color(Token::Ink3)),
        )
        .child(
            text(widget.title())
                .number_of_lines(1u32)
                .truncate()
                .min_w(px(0.0))
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink)),
        )
        .claim_touches(TouchClaim::on_start().ask(|request: &ClaimRequest| request.claim()))
        .keep_touches(true)
        .on_responder_grant(move |_: &ResponderEvent| {
            moving.set(Some(Moving { widget, drop: None }));
            measure(app, cells.clone(), measured.clone());
        })
        .on_responder_move(move |e: &ResponderEvent| follow(e.page.x.0, e.page.y.0))
        .on_responder_release(move |e: &ResponderEvent| {
            if let Some(drop) = drop_at(&dropped.borrow(), widget, e.page.x.0, e.page.y.0) {
                app.board.change(|b| b.drop_on(widget, drop));
            }
            moving.set(None);
        })
        .on_responder_terminate(move |_: &ResponderEvent| moving.set(None))
        .into_element()
}

/// Measures every widget on the board where the pointer reads it.
fn measure(app: AppState, cells: Cells, into: Rc<RefCell<Vec<(Widget, Frame)>>>) {
    let asked: Vec<_> = cells
        .borrow()
        .iter()
        .filter(|(w, _)| app.board.board.with_untracked(|b| b.has(**w)))
        .map(|(w, at)| (*w, at.measure()))
        .collect();
    spawn(async move {
        let mut frames = Vec::with_capacity(asked.len());
        for (widget, answer) in asked {
            if let Some(m) = answer.await {
                frames.push((
                    widget,
                    Frame {
                        x: m.page.x.0,
                        y: m.page.y.0,
                        w: m.frame.size.width.0,
                        h: m.frame.size.height.0,
                    },
                ));
            }
        }
        *into.borrow_mut() = frames;
    });
}

/// The right edge, dragged to resize a column at a time.
fn edge(app: AppState, widget: Widget, item: ReadSignal<Item>, width: Signal<f32>) -> AnyElement {
    // The span the drag began at and one of its columns, in points.
    let start: Rc<Cell<(u8, f32)>> = Rc::new(Cell::new((0, 0.0)));
    let began = start.clone();
    view()
        .test_id(&format!("board-resize-{}", widget.slug()))
        .absolute()
        .top(px(30.0))
        .bottom(px(0.0))
        .right(px(0.0))
        .w(px(14.0))
        .items_center()
        .justify_center()
        .cursor(Cursor::ColResize)
        .label(format!("{}: drag to resize", widget.title()))
        .child(
            view()
                .w(px(4.0))
                .h(px(28.0))
                .rounded(px(2.0))
                .bg(color(Token::Focus)),
        )
        .claim_touches(TouchClaim::on_start().ask(|request: &ClaimRequest| request.claim()))
        .keep_touches(true)
        .on_responder_grant(move |_: &ResponderEvent| {
            let span = item.get_untracked().span;
            let grid = width.get_untracked();
            let shown = shown_span(span, grid);
            let column = grid * f32::from(shown) / f32::from(COLUMNS) / f32::from(span.max(1));
            began.set((span, column));
        })
        .on_responder_move(move |e: &ResponderEvent| {
            let (from, column) = start.get();
            let span = dragged_span(widget, from, e.gesture.delta.x.0, column);
            app.board.shape(|b| b.resize(widget, span));
        })
        .on_responder_release(move |_: &ResponderEvent| app.board.save())
        .on_responder_terminate(move |_: &ResponderEvent| app.board.save())
        .into_element()
}

/// A small button of a widget's head: `glyph` shown, `spoken` said, `btn-board-<what>-<slug>`.
fn tool(
    glyph: &'static str,
    spoken: &'static str,
    what: &'static str,
    widget: Widget,
    press: impl Fn() + 'static,
) -> AnyElement {
    view()
        .test_id(&format!("btn-board-{what}-{}", widget.slug()))
        .role(AccessibilityRole::Button)
        .label(format!("{spoken}: {}", widget.title()))
        .accessible(true)
        .cursor_pointer()
        .flex_shrink_0()
        .w(px(24.0))
        .h(px(24.0))
        .rounded(px(5.0))
        .items_center()
        .justify_center()
        .hover(|s| s.bg(color(Token::Hover)))
        .on_press(press)
        .child(
            text(glyph)
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink2)),
        )
        .into_element()
}
