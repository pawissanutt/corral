//! How a form's text field and its caption look on every form, shared by the operation
//! composer's fields and its tag field.

use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The smallest pressable height on the phone.
const TARGET: f32 = 44.0;

/// A field's caption above it.
pub fn caption(form: Form, label: &'static str) -> Text {
    let caption = text(label)
        .number_of_lines(1u32)
        .text_color(color(Token::Ink2));
    match form {
        Form::Terminal => caption,
        Form::Desktop => caption
            .font_family(FONT_TEXT)
            .text_size(px(12.0))
            .font_weight(FontWeight::SEMIBOLD),
        Form::Phone => caption
            .font_family(FONT_TEXT)
            .text_size(px(14.0))
            .font_weight(FontWeight::SEMIBOLD),
    }
}

/// The field's colours, border and size on `form`; `multiline` is a prose field.
pub fn look(entry: Input, form: Form, multiline: bool) -> Input {
    let entry = entry
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT)
        .border_1()
        .border_color(color(Token::Rule2))
        .focus(|s| s.border_color(color(Token::Focus)));
    match (form, multiline) {
        (Form::Terminal, false) => entry.h(term::rows(1.0)).px(term::cells(1.0)),
        (Form::Terminal, true) => entry
            .min_h(term::rows(3.0))
            .max_h(term::rows(8.0))
            .px(term::cells(1.0)),
        (Form::Desktop, false) => entry
            .font_family(FONT_TEXT)
            .text_size(px(13.0))
            .h(px(32.0))
            .px(px(10.0))
            .rounded(px(6.0)),
        (Form::Desktop, true) => entry
            .font_family(FONT_TEXT)
            .text_size(px(13.0))
            .line_height(px(19.0))
            .min_h(px(96.0))
            .max_h(px(288.0))
            .px(px(10.0))
            .py(px(7.0))
            .rounded(px(6.0)),
        (Form::Phone, false) => entry
            .font_family(FONT_TEXT)
            .text_size(px(16.0))
            .min_h(px(TARGET))
            .px(px(12.0))
            .rounded(px(10.0)),
        (Form::Phone, true) => entry
            .font_family(FONT_TEXT)
            .text_size(px(16.0))
            .line_height(px(22.0))
            .min_h(px(TARGET * 2.5))
            .max_h(px(240.0))
            .px(px(12.0))
            .py(px(10.0))
            .rounded(px(10.0)),
    }
}
