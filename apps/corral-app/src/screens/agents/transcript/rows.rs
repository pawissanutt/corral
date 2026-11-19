//! The transcript's quiet rows and blocks: marker rows (a glyph that says the state without its
//! colour, then muted words; only a failure in the failure colour), the compaction and branch
//! dividers (a rule with words in it, earlier messages readable above), presenter cards, images
//! and an assistant turn's footer. Each opens, where it has more, into its sections.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::transcript::{Card, Divider, Footer, Marker, Picture};
use crate::screens::markdown::markdown;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::run;
use super::parts::{disclosure, hidden, pressable, sections, small, state};

/// The widest a recorded image is drawn on the desktop, and its shape until it has loaded.
const PICTURE_MAX: f32 = 480.0;
const PICTURE_RATIO: f32 = 4.0 / 3.0;

/// Where an opened row's sections sit: under the row's words.
fn opened(app: AppState, body: AnyElement) -> AnyElement {
    if app.form == Form::Terminal {
        view().pl(term::cells(2.0)).child(body).into_element()
    } else {
        view()
            .pl(px(18.0))
            .pt(px(4.0))
            .pb(px(6.0))
            .child(body)
            .into_element()
    }
}

/// A quiet row: the glyph in its state colour, the words muted (a failure's in its colour),
/// the size still to fetch; opened, its full text and facts.
pub(super) fn marker(app: AppState, marker: &Marker, n: usize) -> AnyElement {
    let form = app.form;
    let glyph_ink = if marker.hue == Hue::Neutral {
        color(Token::Ink3)
    } else {
        hue_ink(marker.hue)
    };
    let words_ink = if marker.hue == Hue::Fail {
        Token::Fail
    } else {
        Token::Ink3
    };
    let extra = hidden(None, &marker.more).map(|size| format!(" · {size}"));
    let row = if form == Form::Terminal {
        view()
            .flex_row()
            .child(text(marker.glyph).flex_shrink_0().text_color(glyph_ink))
            .child(term::space(1.0))
            .child(
                text(format!("{}{}", marker.words, extra.unwrap_or_default()))
                    .flex_1()
                    .min_w(px(0.0))
                    .text_color(color(words_ink)),
            )
    } else {
        view()
            .flex_row()
            .items_start()
            .gap(px(6.0))
            .px(px(4.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .child(
                text(marker.glyph)
                    .flex_shrink_0()
                    .w(px(12.0))
                    .font_family(FONT_MONO)
                    .text_size(px(12.0))
                    .line_height(px(17.0))
                    .text_color(glyph_ink),
            )
            .child(
                small(app, marker.words.clone(), words_ink)
                    .line_height(px(17.0))
                    .flex_1()
                    .min_w(px(0.0)),
            )
            .children(extra.map(|size| small(app, size, Token::Ink4).flex_shrink_0()))
            .when(marker.action.is_some(), |row| {
                row.child(disclosure(app, marker.open))
            })
    };
    view()
        .min_w(px(0.0))
        .child(pressable(
            app,
            row,
            marker.action.as_ref(),
            marker.words.clone(),
            marker.open,
            n,
        ))
        .when(marker.open, |block| {
            block.child(opened(app, sections(app, &marker.body)))
        })
        .children(state(app, &marker.more, n, false))
        .into_element()
}

/// A rule across the transcript with the words in it, the note under it; opened, the summary.
pub(super) fn divider(app: AppState, divider: &Divider, n: usize) -> AnyElement {
    let form = app.form;
    let size = hidden(None, &divider.more).map(|size| format!(" · {size}"));
    let words = format!("{}{}", divider.words, size.unwrap_or_default());
    let row = if form == Form::Terminal {
        view()
            .flex_row()
            .child(
                view()
                    .w(term::cells(2.0))
                    .flex_shrink_0()
                    .child(term::hrule()),
            )
            .child(term::space(1.0))
            .child(term::quiet(words.clone()))
            .child(term::space(1.0))
            .child(view().flex_1().min_w(term::cells(2.0)).child(term::hrule()))
    } else {
        let rule = || {
            view()
                .flex_1()
                .min_w(px(16.0))
                .h(px(1.0))
                .bg(color(Token::Rule2))
        };
        view()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .py(px(6.0))
            .rounded(px(4.0))
            .child(rule())
            .child(
                small(app, words.clone(), Token::Ink3)
                    .number_of_lines(1u32)
                    .flex_shrink(1.0)
                    .min_w(px(0.0)),
            )
            .child(disclosure(app, divider.open))
            .child(rule())
    };
    view()
        .min_w(px(0.0))
        .when(form != Form::Terminal, |block| block.my(px(6.0)))
        .child(pressable(
            app,
            row,
            Some(&divider.action),
            words,
            divider.open,
            n,
        ))
        .child(
            small(app, divider.note.clone(), Token::Ink3)
                .when(form != Form::Terminal, |note| note.text_center()),
        )
        .when(divider.open, |block| {
            block.child(opened(app, sections(app, &divider.body)))
        })
        .children(state(app, &divider.more, n, false))
        .into_element()
}

/// A presenter's card: its title on the header row (pressing it opens the data), its text as
/// rendered Markdown, then its data while open. A terminal frames it with the title in its line.
pub(super) fn card(app: AppState, card: &Card, n: usize) -> AnyElement {
    let form = app.form;
    let size = hidden(None, &card.more).map(|size| format!(" · {size}"));
    let hint = card
        .action
        .as_ref()
        .map(|_| format!("data{}", size.unwrap_or_default()));
    let said = (!card.text.trim().is_empty()).then(|| markdown(app, &card.text, false));
    let data = card.open.then(|| sections(app, &card.body));
    let progress = state(app, &card.more, n, false);
    if form == Form::Terminal {
        let title = view()
            .flex_row()
            .child(
                text(card.title.clone())
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink2)),
            )
            .children(hint.map(|hint| {
                view()
                    .flex_row()
                    .child(term::space(1.0))
                    .child(term::quiet(hint))
                    .child(term::space(1.0))
                    .child(disclosure(app, card.open))
            }));
        return term::framed(
            view()
                .px(term::cells(1.0))
                .children(said)
                .children(data)
                .children(progress),
        )
        .child(term::top_title(pressable(
            app,
            title,
            card.action.as_ref(),
            card.title.clone(),
            card.open,
            n,
        )))
        .into_element();
    }
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(6.0))
        .bg(color(Token::Surface2))
        .child(
            text(card.title.clone())
                .flex_1()
                .min_w(px(0.0))
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink2)),
        )
        .children(hint.map(|hint| {
            view()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .flex_shrink_0()
                .child(small(app, hint, Token::Ink3))
                .child(disclosure(app, card.open))
        }));
    view()
        .mt(px(4.0))
        .mb(px(6.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .bg(color(Token::Surface))
        .overflow_hidden()
        .child(pressable(
            app,
            head,
            card.action.as_ref(),
            card.title.clone(),
            card.open,
            n,
        ))
        .children(said.map(|said| view().px(px(12.0)).py(px(8.0)).child(said)))
        .children(data.map(|data| view().px(px(12.0)).pb(px(10.0)).child(data)))
        .children(progress.map(|progress| view().px(px(12.0)).pb(px(8.0)).child(progress)))
        .into_element()
}

/// A recorded image. Desktop and phone draw it at its own proportions on a hairline frame,
/// captioned with its type and size; a terminal names it in one row. Until its complete bytes
/// are here it is named, with a control to fetch it.
pub(super) fn picture(app: AppState, picture: &Picture, n: usize) -> AnyElement {
    let form = app.form;
    let fetch = picture.action.clone().filter(|_| form != Form::Terminal);
    let control = fetch.map(|action| {
        let size = picture.more.size().unwrap_or_default().to_owned();
        button(
            app,
            format!("Show image · {size}"),
            ButtonKind::Quiet,
            format!("btn-expand-{n}"),
            move || run(app, action.clone()),
        )
    });
    let caption = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .child(small(app, format!("▣ {}", picture.label), Token::Ink3))
        .children(
            picture
                .note
                .clone()
                .map(|note| small(app, note, Token::Held)),
        );
    let drawn = picture
        .uri
        .clone()
        .filter(|_| form != Form::Terminal)
        .map(|uri| {
            let ratio = signal(PICTURE_RATIO);
            view()
                .when(form == Form::Desktop, |frame| frame.max_w(px(PICTURE_MAX)))
                .mb(px(4.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(color(Token::Rule))
                .bg(color(Token::BgSunk))
                .overflow_hidden()
                .child(
                    image(uri)
                        .w_full()
                        .aspect_ratio(move || ratio.get())
                        .object_fit(ResizeMode::Contain)
                        .role(AccessibilityRole::Image)
                        .label(picture.label.clone())
                        .on_load(move |size: &Size<DevicePixels>| {
                            if size.width.0 > 0 && size.height.0 > 0 {
                                ratio.set(size.width.0 as f32 / size.height.0 as f32);
                            }
                        }),
                )
        });
    view()
        .min_w(px(0.0))
        .when(form != Form::Terminal, |block| block.my(px(4.0)))
        .children(drawn)
        .child(caption)
        .children(control.map(|control| view().flex_row().child(control)))
        .children(state(app, &picture.more, n, false))
        .into_element()
}

/// The turn's foot: model, an unusual stop and usage, quiet; pressing it shows every fact the
/// turn recorded.
pub(super) fn footer(app: AppState, footer: &Footer, n: usize) -> AnyElement {
    let form = app.form;
    let size = hidden(None, &footer.more).map(|size| format!(" · {size}"));
    let words = format!("{}{}", footer.words, size.unwrap_or_default());
    let row = view()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .when(form != Form::Terminal, |row| {
            row.py(px(2.0)).rounded(px(4.0))
        })
        .child(small(app, words, Token::Ink3).min_w(px(0.0)))
        .when(footer.action.is_some(), |row| {
            row.child(disclosure(app, footer.open))
        });
    view()
        .test_id(&format!("item-{n}"))
        .min_w(px(0.0))
        .when(form != Form::Terminal, |block| block.mt(px(2.0)))
        .child(pressable(
            app,
            row,
            footer.action.as_ref(),
            footer.words.clone(),
            footer.open,
            n,
        ))
        .when(footer.open, |block| {
            block.child(opened(app, sections(app, &footer.body)))
        })
        .children(state(app, &footer.more, n, false))
        .into_element()
}
