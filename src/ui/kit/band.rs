//! The handover band (`.band` on desktop, `.pband` on the phone): one section of a handover
//! sheet, Situation, Background, Assessment, Recommendation, Thread or History, led by its
//! letter and word so the sheet reads as one form across devices.

use nsh_core::prelude::*;

use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// Width of the desktop band's ruled margin, which holds the letter and the word.
const MARGIN: f32 = 44.0;
/// The margin's word: 10 pt capitals, spaced .14 em.
const WORD_SIZE: f32 = 10.0;
const WORD_TRACKING: f32 = 1.4;

/// A handover band: `letter` ("S") and `word` ("Situation") beside or above `content`.
/// Desktop: a 44 pt margin on the sunken ground, ruled off on its right, the letter in 17 pt
/// bold and the word turned to read upwards in spaced capitals under it; the content padded
/// 11 22 12 beside it; a rule under the band. `note` is not shown on desktop (the mockup's desktop
/// bands have none). Phone: a page band (padding 14 18 16, a rule under it) headed by
/// [`band_head`] with the letter, the word and the note. Terminal: the letter in a three-Cell
/// margin ruled off by a joined `│`, the word and note as the content's first row, a joined
/// rule under the band. The band is a region labelled by its word; the margin is hidden from
/// screen readers, which hear the word once.
pub fn band(
    app: AppState,
    letter: &'static str,
    word: impl Into<SharedString>,
    note: Option<String>,
    content: AnyElement,
) -> AnyElement {
    let word: SharedString = word.into();
    let region = view().role(AccessibilityRole::Region).label(word.clone());
    match app.form {
        Form::Desktop => region
            .flex_row()
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(
                view()
                    .w(px(MARGIN))
                    .flex_shrink_0()
                    .self_stretch()
                    .items_center()
                    .gap(px(6.0))
                    .pt(px(14.0))
                    .bg(color(Token::BgSunk))
                    .border_r(px(1.0))
                    .border_r_color(color(Token::Rule))
                    .accessibility_elements_hidden(true)
                    .child(
                        text(letter)
                            .font_family(FONT_TEXT)
                            .text_size(px(17.0))
                            .line_height(px(17.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(Token::Ink2)),
                    )
                    .child(upright_word(word)),
            )
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .px(px(22.0))
                    .pt(px(11.0))
                    .pb(px(12.0))
                    .child(content),
            )
            .into_element(),
        Form::Phone => region
            .px(px(18.0))
            .pt(px(14.0))
            .pb(px(16.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(
                view()
                    .mb(px(8.0))
                    .child(band_head(app, Some(letter), word, note, Hue::Neutral)),
            )
            .child(content)
            .into_element(),
        Form::Terminal => region
            .child(
                view()
                    .flex_row()
                    .child(
                        view()
                            .w(term::cells(3.0))
                            .flex_shrink_0()
                            .flex_row()
                            .accessibility_elements_hidden(true)
                            .child(term::space(1.0))
                            .child(
                                text(letter)
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(color(Token::Ink)),
                            ),
                    )
                    .child(term::vrule())
                    .child(
                        view()
                            .flex_1()
                            .min_w(px(0.0))
                            .px(term::cells(1.0))
                            .child(word_row(word, note))
                            .child(content),
                    ),
            )
            .child(term::hrule())
            .into_element(),
    }
}

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
            .when_some(note, |row, note| row.child(term::quiet(note)))
            .into_element(),
    }
}

/// The desktop margin's word, turned a quarter to read upwards. A turned text keeps its
/// unturned box in the layout, so the box is measured and the holder takes the word's length as
/// its height, the word centred on it; the word shows once measured, never at its unturned place.
fn upright_word(word: SharedString) -> AnyElement {
    let measured = signal(None::<(f32, f32)>);
    let length = move || measured.get().map_or(0.0, |(width, _)| width);
    view()
        .w_full()
        .h(move || px(length()))
        .child(
            text(word)
                .absolute()
                .number_of_lines(1u32)
                .uppercase()
                .font_family(FONT_TEXT)
                .text_size(px(WORD_SIZE))
                .letter_spacing(px(WORD_TRACKING))
                .text_color(color(Token::Ink4))
                .left(move || {
                    px(measured
                        .get()
                        .map_or(0.0, |(width, _)| (MARGIN - width) / 2.0))
                })
                .top(move || {
                    px(measured
                        .get()
                        .map_or(0.0, |(width, height)| (width - height) / 2.0))
                })
                .rotate(deg(-90.0))
                .on_layout(move |frame: &Bounds<Points>| {
                    let size = (frame.size.width.0, frame.size.height.0);
                    if measured.get_untracked() != Some(size) {
                        measured.set(Some(size));
                    }
                })
                .style_when(move || measured.get().is_none(), |s| s.invisible()),
        )
        .into_element()
}

/// The terminal band's first row: its word in bold capitals and the note quiet at the end.
fn word_row(word: SharedString, note: Option<String>) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .accessibility_elements_hidden(true)
        .child(
            text(word)
                .flex_shrink_0()
                .number_of_lines(1u32)
                .uppercase()
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .child(view().flex_1().min_w(px(0.0)))
        .when_some(note, |row, note| row.child(term::quiet(note)))
        .into_element()
}
