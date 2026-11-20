//! Small labels that carry a state: the chip, the verdict stamp, the "illustrative" tag and the
//! count pill.
//!
//! In a terminal a fill is a band (the standard grey on 16 colours), so a chip or stamp keeps
//! its words in the state's colour on that band; a stamp is also bold, so it reads as a verdict
//! without colour.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::kit::marks::{hue_fill, hue_ink};
use crate::ui::term::cells;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The ink of a chip's words: the hue's colour, ink-2 for a neutral chip (ink in a terminal,
/// where ink-2 fades and would read as disabled).
fn chip_ink(form: Form, hue: Hue) -> AdaptiveColor {
    match (hue, form) {
        (Hue::Neutral, Form::Terminal) => color(Token::Ink),
        (Hue::Neutral, _) => color(Token::Ink2),
        _ => hue_ink(hue),
    }
}

/// A chip: the label on the hue's quiet fill in the hue's colour (`.chip.you`, `.chip.held` ...);
/// [`Hue::Neutral`] is ink-2 on surface-2. Desktop 18 pt high, 11 pt text, 4 pt corners; phone
/// 20 pt, 12 pt text; terminal the label on a band with a Cell either side. One line, never cut.
pub fn chip(app: AppState, label: impl Into<String>, hue: Hue) -> AnyElement {
    let form = app.form;
    let label: String = label.into();
    let words = text(label)
        .number_of_lines(1u32)
        .text_color(chip_ink(form, hue));
    let shell = view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .bg(hue_fill(hue));
    match form {
        Form::Terminal => shell.px(cells(1.0)).child(words).into_element(),
        Form::Desktop | Form::Phone => {
            let (height, size) = if form == Form::Phone {
                (20.0, 12.0)
            } else {
                (18.0, 11.0)
            };
            shell
                .h(px(height))
                .px(px(6.0))
                .rounded(px(4.0))
                .child(words.font_family(FONT_TEXT).text_size(px(size)))
                .into_element()
        }
    }
}

/// The tag that marks invented content (`.illus`): "illustrative", "tail illustrative",
/// "machine name illustrative". Italic, faint ink-4, desktop 10.5 pt, phone 12 pt; terminal
/// italic ink-3, as ink-4 would vanish there. A long note wraps inside its parent instead of
/// running past it; a short tag keeps its natural width.
pub fn illus(app: AppState, label: impl Into<String>) -> AnyElement {
    let form = app.form;
    let label: String = label.into();
    let words = text(label).italic().flex_shrink(1.0).min_w(px(0.0));
    match form {
        Form::Terminal => words.text_color(color(Token::Ink3)).into_element(),
        Form::Desktop | Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 12.0 } else { 10.5 }))
            .letter_spacing(px(0.1))
            .text_color(color(Token::Ink4))
            .into_element(),
    }
}
