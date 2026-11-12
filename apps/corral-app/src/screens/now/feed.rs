//! The changes feed's rows, shared by Now's "Since your last visit" and "Parked" panels and the
//! Changes destination: time, a mark per kind of change, the task id and the event, newest first;
//! the per-device seen line between what is new and what was seen.

use nsh_core::prelude::*;

use crate::model::board::{ChangeKind, Mark};
use crate::model::work::{self, FeedRow, HoldRow};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::marks::{dot, hue_ink, mark};
use crate::ui::kit::text::rich_small;
use crate::ui::term::{cells, hrule};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::parts::act;

const RING: f32 = 2.0;

/// The mark of a change: a stage's mark, the worker's start glyph, a note's dot, or the needs-you
/// dot of a message.
fn change_mark(app: AppState, kind: ChangeKind) -> AnyElement {
    match kind {
        ChangeKind::Mark(state) => mark(app, state, Hue::Neutral),
        ChangeKind::Note => mark(app, Mark::Na, Hue::Neutral),
        ChangeKind::Message => dot(app, Hue::You),
        ChangeKind::Start => {
            let glyph = text(work::START_MARK)
                .number_of_lines(1u32)
                .text_color(hue_ink(Hue::Worker))
                .accessibility_elements_hidden(true);
            if app.form == Form::Terminal {
                glyph.into_element()
            } else {
                glyph
                    .font_family(FONT_TEXT)
                    .text_size(px(10.0))
                    .into_element()
            }
        }
    }
}

fn mono(form: Form, content: impl Into<SharedString>, size: f32) -> AnyElement {
    let words = text(content.into())
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    if form == Form::Terminal {
        words.into_element()
    } else {
        words
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(size))
            .into_element()
    }
}

fn column(form: Form, points: f32, terminal_cells: f32, child: AnyElement) -> AnyElement {
    view()
        .w(if form == Form::Terminal {
            cells(terminal_cells)
        } else {
            px(points)
        })
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .child(child)
        .into_element()
}

/// One event (`.ev`): desktop a 26 pt line, time 40, mark 14, id 50, the event cut to fit, new
/// events in ink, seen ones in ink-2 and historical ones faint; terminal the same in Cells; phone
/// the mock's `.prow`: time, id and event, the mark at the trailing edge, at least 48 pt with a
/// hairline under it. A row with an action opens its task; `test_id` names it.
pub(crate) fn row(app: AppState, event: &FeedRow, test_id: String) -> AnyElement {
    let form = app.form;
    let change = event.change;
    let ink = color(if event.dim {
        Token::Ink3
    } else if event.unseen || form == Form::Terminal {
        Token::Ink
    } else {
        Token::Ink2
    });
    let words = text(change.text)
        .flex_1()
        .min_w(px(0.0))
        .number_of_lines(1u32)
        .truncate()
        .text_color(ink);
    let label = format!("{} {} {}", change.at, change.id, change.text);
    let base = view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .label(label)
        .test_id(&test_id);
    let base = match event.action {
        Some(action) => base
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .on_press(move || act(app, action))
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-RING))
            }),
        None => base,
    };
    match form {
        Form::Phone => base
            .min_h(px(48.0))
            .px(px(18.0))
            .py(px(10.0))
            .gap(px(10.0))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .active(|s| s.bg(color(Token::Hover)))
            .child(column(form, 44.0, 0.0, mono(form, change.at, 12.5)))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex_row()
                    .items_baseline()
                    .gap(px(6.0))
                    .child(mono(form, change.id, 13.0))
                    .child(words.font_family(FONT_TEXT).text_size(px(14.5))),
            )
            .child(column(form, 14.0, 0.0, change_mark(app, change.kind)))
            .into_element(),
        Form::Desktop => base
            .h(px(26.0))
            .px(px(16.0))
            .gap(px(8.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .child(column(form, 40.0, 0.0, mono(form, change.at, 11.0)))
            .child(column(form, 14.0, 0.0, change_mark(app, change.kind)))
            .child(column(form, 50.0, 0.0, mono(form, change.id, 11.0)))
            .child(words.font_family(FONT_TEXT).text_size(px(12.0)))
            .into_element(),
        Form::Terminal => base
            .h(px(16.0))
            .px(cells(1.0))
            .gap(px(8.0))
            .child(column(form, 0.0, 5.0, mono(form, change.at, 0.0)))
            .child(column(form, 0.0, 1.0, change_mark(app, change.kind)))
            .child(column(form, 0.0, 5.0, mono(form, change.id, 0.0)))
            .child(words)
            .into_element(),
    }
}

/// One hold, in the feed's columns: when the hold began, its mark, the id, the title with its
/// attribution dimmed after it. Not pressable, as in the mock.
pub(crate) fn hold(app: AppState, held: &HoldRow) -> AnyElement {
    let form = app.form;
    let label = format!("{} {} {}", held.since, held.id, held.title);
    let row = view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .overflow_hidden()
        .label(label)
        .child(column(
            form,
            40.0,
            5.0,
            mono(form, held.since.clone(), 11.0),
        ))
        .child(column(form, 14.0, 1.0, mark(app, held.mark, Hue::Neutral)))
        .child(column(form, 50.0, 5.0, mono(form, held.id, 11.0)))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .child(rich_small(app, &held.text)),
        );
    match form {
        Form::Terminal => row.h(px(16.0)).px(cells(1.0)).gap(px(8.0)),
        Form::Desktop | Form::Phone => row.h(px(26.0)).px(px(16.0)).gap(px(8.0)),
    }
    .into_element()
}

/// The per-device seen line (`.seenline`): the model's words in faint spaced capitals between
/// two lines.
pub(crate) fn seen_line(app: AppState, label: &'static str) -> AnyElement {
    let form = app.form;
    if form == Form::Terminal {
        return view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(view().flex_1().min_w(px(0.0)).child(hrule()))
            .child(
                text(label)
                    .number_of_lines(1u32)
                    .flex_shrink_0()
                    .text_color(color(Token::Ink3)),
            )
            .child(view().flex_1().min_w(px(0.0)).child(hrule()))
            .into_element();
    }
    let rule = || {
        view()
            .flex_1()
            .min_w(px(0.0))
            .h(px(1.0))
            .bg(color(Token::Rule2))
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(16.0))
        .py(px(6.0))
        .role(AccessibilityRole::Header)
        .child(rule())
        .child(
            text(label)
                .uppercase()
                .number_of_lines(1u32)
                .flex_shrink_0()
                .font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 12.0 } else { 10.5 }))
                .letter_spacing(px(10.5 * 0.07))
                .text_color(color(Token::Ink4)),
        )
        .child(rule())
        .into_element()
}
