//! Styled text: the model's [`Rich`] sentences, and the plain, mono, dim, section-caption and
//! label-value pieces of the mock's CSS (`.mono`, `.dim`, `.h3`, `.kv`).
//!
//! Sizes follow the form: desktop body 13 pt, phone 16 pt, terminal one row per line in the
//! terminal's font. A terminal fades faint neutral inks on its own, so dim text stays readable
//! there and needs no other treatment.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::text::{Rich, Span};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term::{cells, hrule};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

/// Body line height as a multiple of the size: the desktop's 1.45; on a phone the 1.5 its
/// paragraphs read at (`.pband p`).
fn leading(form: Form) -> f32 {
    match form {
        Form::Phone => 1.5,
        Form::Desktop | Form::Terminal => 1.45,
    }
}

/// `.mono` is 0.93 of the text around it.
const MONO_SCALE: f32 = 0.93;

/// The secondary size of a form: desktop 12 pt, phone 13.5 pt, terminal its one size.
fn small_size(form: Form) -> f32 {
    match form {
        Form::Desktop => 12.0,
        Form::Phone => 13.5,
        Form::Terminal => text_size(form),
    }
}

/// A paragraph of the model's [`Rich`] text at the form's body size in ink, wrapping like a
/// paragraph. Each span keeps its styles: bold, mono (the mono font at 0.93 of the size),
/// dim (ink-3), struck through, its hue's colour (over dim), and a link (underlined, pressing it
/// calls `app.open(page)`). In a terminal every link is also a stop of the keyboard focus.
pub fn rich(app: AppState, rich: &Rich) -> AnyElement {
    paragraph(app, rich, text_size(app.form), Token::Ink)
}

/// [`rich`] at the form's secondary size in ink-2: goal lines, a card's meta line, notices.
pub fn rich_small(app: AppState, rich: &Rich) -> AnyElement {
    paragraph(app, rich, small_size(app.form), Token::Ink2)
}

/// [`rich`] at `size` points with `ink` as the base colour of runs without a hue or dim of their
/// own: a lede at 13.5 pt, a phone band paragraph at 15.5 pt. A terminal keeps its one size and
/// takes only the ink.
pub fn rich_at(app: AppState, rich: &Rich, size: f32, ink: Token) -> AnyElement {
    paragraph(app, rich, size, ink)
}

fn paragraph(app: AppState, rich: &Rich, size: f32, ink: Token) -> AnyElement {
    let form = app.form;
    let root = text("")
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(color(ink));
    let root = if form == Form::Terminal {
        root
    } else {
        root.font_family(FONT_TEXT)
            .text_size(px(size))
            .line_height(px((size * leading(form)).round()))
    };
    root.children(rich.iter().map(|run| span_of(app, run, size)))
        .into_element()
}

fn span_of(app: AppState, run: &Span, size: f32) -> Text {
    let form = app.form;
    let mut piece = text(run.text.clone());
    if run.bold {
        piece = piece.font_weight(FontWeight::BOLD);
    }
    if run.mono && form != Form::Terminal {
        let mono = size * MONO_SCALE;
        piece = piece
            .font_family(FONT_MONO)
            .text_size(px(mono))
            .letter_spacing(px(-0.01 * mono));
    }
    if run.hue != Hue::Neutral {
        piece = piece.text_color(hue_ink(run.hue));
    } else if run.dim {
        piece = piece.text_color(color(Token::Ink3));
    }
    if run.strike {
        piece = piece.line_through();
    }
    if let Some(page) = run.link {
        piece = piece
            .underline()
            .text_decoration_color(color(Token::Ink).opacity(0.35))
            .underline_offset(px(2.0))
            .cursor_pointer()
            .role(AccessibilityRole::Link)
            .on_press(move || app.open(page));
    }
    piece
}

/// Body text at the form's size in ink, one paragraph that wraps (chain `.truncate()` for one
/// line).
pub fn plain(app: AppState, content: impl Into<SharedString>) -> Text {
    let form = app.form;
    let words = text(content.into()).text_color(color(Token::Ink));
    if form == Form::Terminal {
        words
    } else {
        words.font_family(FONT_TEXT).text_size(px(text_size(form)))
    }
}

/// An id, SHA, path, command or time in the mono font with tabular figures, at 0.93 of the
/// secondary size (desktop 11.5 pt as `.row .id`, phone 13 pt), in ink. A terminal shows it in
/// its own font and size.
pub fn mono(app: AppState, content: impl Into<SharedString>) -> Text {
    let form = app.form;
    let words = text(content.into()).text_color(color(Token::Ink));
    match form {
        Form::Terminal => words,
        Form::Desktop => words
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(11.5)),
        Form::Phone => words
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(13.0)),
    }
}

/// Secondary words in ink-3 (`.dim`) at the form's secondary size (desktop 12 pt, phone
/// 13.5 pt).
pub fn dim(app: AppState, content: impl Into<SharedString>) -> Text {
    let form = app.form;
    let words = text(content.into()).text_color(color(Token::Ink3));
    if form == Form::Terminal {
        words
    } else {
        words.font_family(FONT_TEXT).text_size(px(small_size(form)))
    }
}

/// A section caption (`.h3`): the label in spaced capitals, ink-3, semibold, then a hairline to
/// the trailing edge, then the optional note in ink-4 (`.h3 .r`). Desktop 11 pt with 18 pt above
/// and 8 pt below; phone 12 pt with 14 pt above and 6 pt below. A terminal draws the label bold
/// and the line as a joined `─` rule, with no space of its own: the page places blank rows.
pub fn h3(app: AppState, label: impl Into<SharedString>, note: Option<String>) -> AnyElement {
    caption(app, label.into(), note, true)
}

/// [`h3`] with no space above it, for the first caption of a region whose own padding already
/// sets it off; the space below stays (8 pt desktop, 6 pt phone).
pub fn h3_flush(app: AppState, label: impl Into<SharedString>, note: Option<String>) -> AnyElement {
    caption(app, label.into(), note, false)
}

fn caption(app: AppState, label: SharedString, note: Option<String>, spaced: bool) -> AnyElement {
    let form = app.form;
    let caption = text(label)
        .uppercase()
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(Token::Ink3));
    let row = view()
        .flex_row()
        .items_center()
        .role(AccessibilityRole::Header);
    if form == Form::Terminal {
        return row
            .gap(px(8.0))
            .child(caption.font_weight(FontWeight::BOLD))
            .child(view().flex_1().min_w(px(0.0)).child(hrule()))
            .children(note.map(|note| {
                text(note)
                    .number_of_lines(1u32)
                    .flex_shrink_0()
                    .text_color(color(Token::Ink3))
            }))
            .into_element();
    }
    let (size, above, below) = if form == Form::Phone {
        (12.0, 14.0, 6.0)
    } else {
        (11.0, 18.0, 8.0)
    };
    row.gap(px(8.0))
        .pt(px(if spaced { above } else { 0.0 }))
        .pb(px(below))
        .child(
            caption
                .font_family(FONT_TEXT)
                .text_size(px(size))
                .font_weight(FontWeight::SEMIBOLD)
                .letter_spacing(px(size * 0.07)),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .h(px(1.0))
                .bg(color(Token::Rule)),
        )
        .children(note.map(|note| {
            text(note)
                .number_of_lines(1u32)
                .flex_shrink_0()
                .font_family(FONT_TEXT)
                .text_size(px(size))
                .text_color(color(Token::Ink4))
        }))
        .into_element()
}

/// One row of a label-value list (`.kv`): the label in ink-3 in a fixed column, the value beside
/// it, top-aligned so a value that wraps keeps its label on its first line. Desktop 76 pt column,
/// 12.5 pt label, 12 pt apart; phone 84 pt, 14 pt; terminal 12 Cells. Stack the rows in a column
/// 5 pt apart (the mock's row gap); size the value's own text to match.
pub fn label_value(app: AppState, label: impl Into<SharedString>, value: AnyElement) -> AnyElement {
    let form = app.form;
    let words = text(label.into())
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    let (column, words) = match form {
        Form::Terminal => (cells(12.0), words),
        Form::Desktop => (px(76.0), words.font_family(FONT_TEXT).text_size(px(12.5))),
        Form::Phone => (px(84.0), words.font_family(FONT_TEXT).text_size(px(14.0))),
    };
    view()
        .flex_row()
        .items_start()
        .gap(px(if form == Form::Terminal { 0.0 } else { 12.0 }))
        .child(view().w(column).flex_shrink_0().child(words))
        .child(view().flex_1().min_w(px(0.0)).child(value))
        .into_element()
}
