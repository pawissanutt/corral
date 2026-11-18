//! What every Settings section shares: the section frame of each form, its spacing and focus
//! ring, and the cells of its tables.

use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

/// The widest a section reads at on the desktop (the mockup's `max-width: 960px`).
const MEASURE: f32 = 960.0;

/// The keyboard focus ring: 2 pt of the focus colour, 2 pt out.
pub fn ring(s: StyleRefinement) -> StyleRefinement {
    s.outline(px(2.0))
        .outline_color(color(Token::Focus))
        .outline_offset(px(2.0))
}

/// Controls in a row: 8 pt apart, two Cells apart in a terminal.
pub fn button_row(app: AppState, buttons: Vec<AnyElement>) -> AnyElement {
    if app.form == Form::Terminal {
        let mut parts = Vec::with_capacity(buttons.len() * 2);
        for (at, button) in buttons.into_iter().enumerate() {
            if at > 0 {
                parts.push(term::space(2.0));
            }
            parts.push(button);
        }
        return view()
            .flex_row()
            .items_center()
            .children(parts)
            .into_element();
    }
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .children(buttons)
        .into_element()
}

/// Vertical room between two parts of a section: `points` on a screen, a blank row in a
/// terminal, where margins inside a frame take no rows.
pub fn space(app: AppState, points: f32) -> AnyElement {
    if app.form == Form::Terminal {
        return term::blank();
    }
    view().h(px(points)).flex_shrink_0().into_element()
}

/// The section's title (`.set h2`), test_id "page-title": 16 pt bold on the desktop; in a
/// terminal the title set into the frame's line. The phone names the section in its pushed
/// head instead.
fn heading(app: AppState, title: &'static str) -> AnyElement {
    let words = text(title)
        .test_id("page-title")
        .role(AccessibilityRole::Header)
        .number_of_lines(1u32)
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    if app.form == Form::Terminal {
        return words.into_element();
    }
    words
        .font_family(FONT_TEXT)
        .text_size(px(16.0))
        .letter_spacing(px(-0.08))
        .into_element()
}

/// A section as each form shows it: on the desktop the title over the parts, `gap` points
/// apart, held to the mockup's measure; in a terminal a framed pane with the title in its top
/// line; on the phone the parts alone, padded as the mockup's page bodies (14 18 24).
pub fn frame(app: AppState, title: &'static str, gap: f32, parts: Vec<AnyElement>) -> AnyElement {
    match app.form {
        Form::Desktop => view()
            .max_w(px(MEASURE))
            .child(heading(app, title))
            .child(view().h(px(gap)).flex_shrink_0())
            .children(parts)
            .into_element(),
        Form::Terminal => term::framed(view().px(term::cells(1.0)).children(parts))
            .child(term::top_title(heading(app, title)))
            .into_element(),
        Form::Phone => view()
            .px(px(18.0))
            .pt(px(14.0))
            .pb(px(24.0))
            .children(parts)
            .into_element(),
    }
}

/// The text size of a table cell (`.qtable`, `.devtable`: 12.5 pt); a step up on the phone,
/// where 12.5 would read small at arm's length.
pub fn cell_size(form: Form) -> f32 {
    match form {
        Form::Desktop => 12.5,
        Form::Phone => 13.5,
        Form::Terminal => text_size(form),
    }
}

/// A table cell's words in `ink`, wrapping; bold when `strong`.
pub fn cell(app: AppState, words: impl Into<SharedString>, ink: Token, strong: bool) -> Text {
    let form = app.form;
    let words = text(words.into())
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(color(ink))
        .when(strong, |t| t.font_weight(FontWeight::BOLD));
    if form == Form::Terminal {
        return words;
    }
    let size = cell_size(form);
    words
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .line_height(px((size * 1.4).round()))
}

/// A table cell's ids, keys or paths in the mono font (`.mono`, 0.93 of the cell's size), in
/// `ink`, wrapping.
pub fn mono_cell(app: AppState, words: impl Into<SharedString>, ink: Token) -> Text {
    let form = app.form;
    let words = text(words.into())
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(color(ink));
    if form == Form::Terminal {
        return words;
    }
    let size = cell_size(form) * 0.93;
    words
        .font_family(FONT_MONO)
        .tabular_nums()
        .text_size(px(size))
        .line_height(px((size * 1.5).round()))
        .letter_spacing(px(-0.01 * size))
}

/// A section caption (`.h3`) with no room above it, as the mockup's first one in a section
/// (`margin-top: 0`), with its optional note at the trailing edge.
pub fn first_heading(app: AppState, label: &'static str, note: Option<&'static str>) -> AnyElement {
    let lift = match app.form {
        Form::Desktop => 18.0,
        Form::Phone => 14.0,
        Form::Terminal => 0.0,
    };
    view()
        .mt(px(-lift))
        .child(h3(app, label, note.map(str::to_owned)))
        .into_element()
}

/// One record of a phone list: what leads its first line, what sits at that line's trailing
/// edge, and the lines under it.
pub struct Record {
    pub lead: AnyElement,
    pub end: Option<AnyElement>,
    pub under: Vec<AnyElement>,
}

/// Records stacked for a narrow phone, where a table's three or four columns would break names
/// and model ids mid-token: each record's lead and trailing control on one line, its details
/// under it at full width, records parted by a hairline.
pub fn records(records: Vec<Record>) -> AnyElement {
    view()
        .min_w(px(0.0))
        .children(records.into_iter().enumerate().map(|(at, record)| {
            view()
                .min_w(px(0.0))
                .py(px(10.0))
                .gap(px(4.0))
                .when(at > 0, |row| {
                    row.border_t_1().border_t_color(color(Token::Rule))
                })
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .min_w(px(0.0))
                        .child(view().flex_1().min_w(px(0.0)).child(record.lead))
                        .children(record.end.map(|end| {
                            view().flex_row().flex_shrink_0().child(end).into_element()
                        })),
                )
                .children(record.under)
                .into_element()
        }))
        .into_element()
}
