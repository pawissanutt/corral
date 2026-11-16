//! The changes feed's rows for the Changes destination: time, a mark per kind of change, the
//! task id and the event, newest first; the per-device seen line between what is new and what
//! was seen.

use nsh_core::prelude::*;

use crate::model::board::{ChangeKind, Mark};
use crate::model::work::{self, FeedRow};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::marks::{dot, hue_ink, mark};
use crate::ui::kit::rows::{measured, project_chip};
use crate::ui::term::{cells, hrule};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::parts::press;

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

/// One event (`.ev`): desktop a 26 pt line, time, mark 14, id 50, the event cut to fit, its
/// project chip when the feed spans projects, then corrald's attribution in faint ink; new
/// events in ink, seen ones in ink-2 and historical ones faint; terminal the same in Cells;
/// phone the mock's `.prow`: time, id and event (the project chip and attribution under it), the
/// mark at the trailing edge, at least 48 pt with a hairline under it. The time column holds
/// `18:40` and `Nov 12` alike. A row with an action opens its task;
/// `test_id` names it.
pub(crate) fn row(app: AppState, event: &FeedRow, test_id: String) -> AnyElement {
    let form = app.form;
    // A failed change keeps the failure's colour in its words too, so it is not read past.
    let failed = event.kind == ChangeKind::Mark(Mark::Fail);
    let ink = if event.dim {
        color(Token::Ink3)
    } else if failed {
        hue_ink(Hue::Fail)
    } else {
        color(if event.unseen || form == Form::Terminal {
            Token::Ink
        } else {
            Token::Ink2
        })
    };
    let words = text(event.text.clone())
        .flex_1()
        .min_w(px(0.0))
        .number_of_lines(1u32)
        .truncate()
        .text_color(ink);
    let by = event.by.clone().map(|by| {
        let words = text(by)
            .number_of_lines(1u32)
            .truncate()
            .text_color(color(Token::Ink3));
        match form {
            Form::Terminal => words.max_w(cells(24.0)),
            Form::Desktop => words
                .max_w(px(220.0))
                .font_family(FONT_TEXT)
                .text_size(px(11.0)),
            Form::Phone => words.font_family(FONT_TEXT).text_size(px(12.0)),
        }
        .into_element()
    });
    // A terminal row's Cells beside the event: time 6, mark 1, id 5, the side padding and the
    // gaps, the attribution after it.
    let room = signal(0.0_f32);
    let reserved = 6.0
        + 1.0
        + 1.0
        + 5.0
        + 2.0
        + event
            .by
            .as_ref()
            .map_or(0.0, |by| by.chars().count().min(24) as f32 + 1.0);
    let project = event
        .project_chip
        .clone()
        .map(|project| project_chip(app, project, room, reserved));
    let label = match &event.project_chip {
        Some(project) => format!("{}, {project}", event.label()),
        None => event.label(),
    };
    let base = view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .label(label)
        .test_id(&test_id);
    let base = match event.action.clone() {
        Some(action) => base
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .on_press(move || press(app, action.clone()))
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
            .child(column(form, 48.0, 0.0, mono(form, event.at.clone(), 12.5)))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(2.0))
                    .child(
                        view()
                            .flex_row()
                            .items_baseline()
                            .gap(px(6.0))
                            .child(mono(form, event.id.clone(), 13.0))
                            .child(words.font_family(FONT_TEXT).text_size(px(14.5))),
                    )
                    .children((project.is_some() || by.is_some()).then(move || {
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .children(project)
                            .children(by)
                    })),
            )
            .child(column(form, 14.0, 0.0, change_mark(app, event.kind)))
            .into_element(),
        Form::Desktop => base
            .h(px(26.0))
            .px(px(16.0))
            .gap(px(8.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .child(column(form, 44.0, 0.0, mono(form, event.at.clone(), 11.0)))
            .child(column(form, 14.0, 0.0, change_mark(app, event.kind)))
            .child(column(form, 50.0, 0.0, mono(form, event.id.clone(), 11.0)))
            .child(words.font_family(FONT_TEXT).text_size(px(12.0)))
            .children(project)
            .children(by)
            .into_element(),
        Form::Terminal => measured(app, base, room)
            .h(px(16.0))
            .px(cells(1.0))
            .gap(px(8.0))
            .child(column(form, 0.0, 6.0, mono(form, event.at.clone(), 0.0)))
            .child(column(form, 0.0, 1.0, change_mark(app, event.kind)))
            .child(column(form, 0.0, 5.0, mono(form, event.id.clone(), 0.0)))
            .child(words)
            .children(project)
            .children(by)
            .into_element(),
    }
}

/// The per-device seen line (`.seenline`): the model's words in faint spaced capitals between
/// two lines.
pub(crate) fn seen_line(app: AppState, label: impl Into<SharedString>) -> AnyElement {
    let label: SharedString = label.into();
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
