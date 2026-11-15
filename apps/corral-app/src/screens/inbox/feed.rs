//! What changed since the last visit, under the Inbox list: one row per change, then the way
//! to all of them.

use nsh_core::prelude::*;

use super::{button, code, press, ring, words};
use crate::model::board::{ChangeKind, Mark};
use crate::model::inbox::{self, FeedRow};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::marks::{dot, hue_ink, mark};
use crate::ui::kit::rows::list_row;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

/// What changed, by its kind: the stage's own mark, a worker start, a note, a message for you.
fn change_mark(app: AppState, kind: ChangeKind) -> AnyElement {
    match kind {
        ChangeKind::Mark(state) => mark(app, state, Hue::Neutral),
        ChangeKind::Note => mark(app, Mark::Na, Hue::Neutral),
        ChangeKind::Message => dot(app, Hue::You),
        ChangeKind::Start => text("▶")
            .when(app.form != Form::Terminal, |t| t.text_size(px(10.0)))
            .text_color(hue_ink(Hue::Worker))
            .accessibility_elements_hidden(true)
            .into_element(),
    }
}

/// A row of the feed. Desktop (`.ev`, 26 pt): time, mark, id, the text in ink while unseen.
/// Phone (`.prow`, 48 pt): time, id and text on one line, the mark at the end. Terminal: the
/// kit's one-line row.
pub(super) fn feed_row(app: AppState, row: FeedRow) -> AnyElement {
    let label = format!("{} {} {}", row.at, row.id, row.text);
    let on_press = press(app, row.action.clone());
    match app.form {
        Form::Terminal => list_row(
            app,
            label,
            row.test_id.clone(),
            || false,
            on_press,
            vec![
                term::quiet(row.at.clone()).flex_shrink_0().into_element(),
                change_mark(app, row.kind),
                term::quiet(row.id.clone()).flex_shrink_0().into_element(),
                text(row.text.clone())
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(color(Token::Ink))
                    .into_element(),
            ],
        ),
        Form::Desktop => view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .h(px(26.0))
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .label(label)
            .test_id(&row.test_id)
            .on_press(on_press)
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(ring)
            .child(view().w(px(40.0)).flex_shrink_0().child(code(
                app,
                row.at.clone(),
                11.0,
                Token::Ink3,
            )))
            .child(
                view()
                    .w(px(14.0))
                    .flex_shrink_0()
                    .items_center()
                    .child(change_mark(app, row.kind)),
            )
            .child(view().w(px(50.0)).flex_shrink_0().child(code(
                app,
                row.id.clone(),
                11.0,
                Token::Ink3,
            )))
            .child(
                words(
                    app,
                    row.text.clone(),
                    12.0,
                    if row.unseen { Token::Ink } else { Token::Ink2 },
                )
                .flex_1()
                .min_w(px(0.0))
                .truncate(),
            )
            .into_element(),
        Form::Phone => view()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .min_h(px(48.0))
            .px(px(18.0))
            .py(px(8.0))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .label(label)
            .test_id(&row.test_id)
            .on_press(on_press)
            .active(|s| s.bg(color(Token::Hover)))
            .focus_visible(ring)
            .child(view().w(px(44.0)).flex_shrink_0().child(code(
                app,
                row.at.clone(),
                12.5,
                Token::Ink3,
            )))
            .child(
                text("")
                    .flex_1()
                    .min_w(px(0.0))
                    .number_of_lines(1u32)
                    .child(code(app, row.id.clone(), 13.5, Token::Ink3))
                    .child(words(app, " ", 14.5, Token::Ink))
                    .child(words(app, row.text.clone(), 14.5, Token::Ink)),
            )
            .child(
                view()
                    .w(px(14.0))
                    .flex_shrink_0()
                    .items_center()
                    .child(change_mark(app, row.kind)),
            )
            .into_element(),
    }
}

/// "All 12 changes ›" under the desktop feed (`.more`: 26 pt, 48 pt in, ink-3); the kit's quiet
/// button in a terminal.
pub(super) fn more_row(app: AppState, all: &inbox::Button) -> AnyElement {
    if app.form != Form::Desktop {
        return view()
            .flex_row()
            .px(term::cells(1.0))
            .child(button(app, all))
            .into_element();
    }
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .h(px(26.0))
        .pl(px(48.0))
        .pr(px(16.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(all.label.clone())
        .test_id("btn-go-changes")
        .on_press(press(app, all.action.clone()))
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(ring)
        .child(words(app, all.label.clone(), 12.0, Token::Ink3))
        .child(icon(Icon::Chevron, 12.0, color(Token::Ink3)))
        .into_element()
}
