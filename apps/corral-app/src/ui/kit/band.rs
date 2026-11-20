//! The handover band (`.band` on desktop, `.pband` on the phone): one section of a handover
//! sheet, Situation, Background, Assessment, Recommendation, Thread or History, led by its
//! letter and word so the sheet reads as one form across devices.

use nsh_core::prelude::*;

use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The head of a band or a page section: an optional letter, the label in capitals, a note at
/// the trailing edge. Desktop: the mockup's `.h3`, 11 pt semibold spaced capitals in the quiet
/// ink (the hue's when not neutral), the letter 13 pt bold before it, a rule running to the
/// note. Phone: `.pband .bh`, 11.5 pt bold spaced capitals, the letter 15 pt bold, the note at
/// the trailing edge. Terminal: the letter and the label in bold, the note quiet at the end.
/// The row has the header role and reads as "<letter> <label>".
pub fn band_head(
    app: AppState,
    letter: Option<&'static str>,
    label: impl Into<SharedString>,
    note: Option<String>,
    hue: Hue,
) -> AnyElement {
    let label: SharedString = label.into();
    let spoken = match letter {
        Some(letter) => format!("{letter} {label}"),
        None => label.to_string(),
    };
    let row = view()
        .flex_row()
        .role(AccessibilityRole::Header)
        .label(spoken);
    let label_ink = match hue {
        Hue::Neutral => color(Token::Ink3),
        _ => hue_ink(hue),
    };
    match app.form {
        Form::Desktop => row
            .items_center()
            .gap(px(8.0))
            .when_some(letter, |row, letter| {
                row.child(
                    text(letter)
                        .font_family(FONT_TEXT)
                        .text_size(px(13.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink2)),
                )
            })
            .child(
                text(label)
                    .flex_shrink_0()
                    .number_of_lines(1u32)
                    .uppercase()
                    .font_family(FONT_TEXT)
                    .text_size(px(11.0))
                    .letter_spacing(px(0.77))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(label_ink),
            )
            .child(view().flex_1().h(px(1.0)).bg(color(Token::Rule)))
            .when_some(note, |row, note| {
                row.child(
                    text(note)
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .truncate()
                        .font_family(FONT_TEXT)
                        .text_size(px(11.0))
                        .text_color(color(Token::Ink4)),
                )
            })
            .into_element(),
        Form::Phone => row
            .items_baseline()
            .gap(px(8.0))
            .when_some(letter, |row, letter| {
                row.child(
                    text(letter)
                        .font_family(FONT_TEXT)
                        .text_size(px(15.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink2)),
                )
            })
            .child(
                text(label)
                    .flex_shrink_0()
                    .number_of_lines(1u32)
                    .uppercase()
                    .font_family(FONT_TEXT)
                    .text_size(px(11.5))
                    .letter_spacing(px(1.15))
                    .font_weight(FontWeight::BOLD)
                    .text_color(label_ink),
            )
            .child(view().flex_1().min_w(px(0.0)))
            .when_some(note, |row, note| {
                row.child(
                    text(note)
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .truncate()
                        .font_family(FONT_TEXT)
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color(Token::Ink3)),
                )
            })
            .into_element(),
        Form::Terminal => row
            .items_center()
            .when_some(letter, |row, letter| {
                row.child(
                    text(letter)
                        .flex_shrink_0()
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink)),
                )
                .child(term::space(1.0))
            })
            .child(
                text(label)
                    .flex_shrink_0()
                    .number_of_lines(1u32)
                    .uppercase()
                    .font_weight(FontWeight::BOLD)
                    .text_color(match hue {
                        Hue::Neutral => color(Token::Ink),
                        _ => hue_ink(hue),
                    }),
            )
            .child(view().flex_1().min_w(px(0.0)))
            .when_some(note, |row, note| row.child(terminal_note(note)))
            .into_element(),
    }
}

/// A terminal band head's note: a Cell after the room before it, so it never meets the word,
/// and cut with an ellipsis when the row runs short, as the word keeps its full length.
fn terminal_note(note: String) -> AnyElement {
    view()
        .flex_row()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .child(term::space(1.0))
        .child(term::quiet(note).flex_shrink(1.0).min_w(px(0.0)).truncate())
        .into_element()
}
