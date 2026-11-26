//! The transcript's quiet rows and blocks: marker rows (a glyph that says the state without its
//! colour, then muted words and the time of day at the right; a failure's sentence in the
//! failure colour with its next step muted under it), the compaction and branch dividers (a
//! rule with words in it, earlier messages readable above), presenter cards and an assistant
//! turn's footer. Each opens, where it has more, into its sections.

use nsh_core::EllipsizeMode;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::transcript::{Card, Divider, Footer, Marker, Reply};
use crate::screens::markdown::markdown;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::parts::{disclosure, hidden, indented, pressable, sections, small, state};

/// A quiet row: the glyph in its state colour, the words muted (a failure's in its colour, at
/// most two rows when they are its own unrecognised words), the size still to fetch and the
/// time of day; under a failure its next step; opened, its full text and facts.
pub(super) fn marker(app: AppState, marker: &Marker, id: &str) -> AnyElement {
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
    let rows = if marker.verbatim { 2u32 } else { 1u32 };
    let row = if form == Form::Terminal {
        view()
            .flex_row()
            .child(text(marker.glyph).flex_shrink_0().text_color(glyph_ink))
            .child(term::space(1.0))
            .child(
                text(format!("{}{}", marker.words, extra.unwrap_or_default()))
                    .flex_1()
                    .min_w(px(0.0))
                    .number_of_lines(if marker.open { 400u32 } else { rows })
                    .text_overflow(EllipsizeMode::Tail)
                    .text_color(color(words_ink)),
            )
            .children(marker.time.clone().map(|time| {
                view()
                    .flex_row()
                    .flex_shrink_0()
                    .child(term::space(2.0))
                    .child(term::quiet(time))
            }))
            .when(marker.action.is_some(), |row| {
                row.child(term::space(1.0))
                    .child(disclosure(app, marker.open))
            })
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
                    .when(!marker.open, |words| {
                        words
                            .number_of_lines(rows)
                            .text_overflow(EllipsizeMode::Tail)
                    })
                    .flex_1()
                    .min_w(px(0.0)),
            )
            .children(extra.map(|size| small(app, size, Token::Ink4).flex_shrink_0()))
            .children(
                marker
                    .time
                    .clone()
                    .map(|time| small(app, time, Token::Ink4).flex_shrink_0().tabular_nums()),
            )
            .when(marker.action.is_some(), |row| {
                row.child(disclosure(app, marker.open))
            })
    };
    let step = marker.step.clone().map(|step| {
        let words = small(app, step, Token::Ink3);
        if form == Form::Terminal {
            view().pl(term::cells(2.0)).child(words).into_element()
        } else {
            view().pl(px(22.0)).pb(px(2.0)).child(words).into_element()
        }
    });
    view()
        .min_w(px(0.0))
        .child(pressable(
            app,
            row,
            marker.action.as_ref(),
            marker.words.clone(),
            marker.open,
            id,
        ))
        .children(step)
        .when(marker.open, |block| {
            block.child(indented(app, sections(app, &marker.body)))
        })
        .children(state(app, &marker.more, id, false))
        .into_element()
}

/// A rule across the transcript with the words in it, the note under it; opened, the summary.
pub(super) fn divider(app: AppState, divider: &Divider, id: &str) -> AnyElement {
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
            id,
        ))
        .child(
            small(app, divider.note.clone(), Token::Ink3)
                .when(form != Form::Terminal, |note| note.text_center()),
        )
        .when(divider.open, |block| {
            block.child(indented(app, sections(app, &divider.body)))
        })
        .children(state(app, &divider.more, id, false))
        .into_element()
}

/// A presenter's card: its title on the header row (pressing it opens the data), its text as
/// rendered Markdown, then its data while open. A terminal frames it with the title in its line.
pub(super) fn card(app: AppState, card: &Card, id: &str) -> AnyElement {
    let form = app.form;
    let size = hidden(None, &card.more).map(|size| format!(" · {size}"));
    let hint = card
        .action
        .as_ref()
        .map(|_| format!("data{}", size.unwrap_or_default()));
    let said = (!card.text.trim().is_empty()).then(|| markdown(app, &card.text, false));
    let data = card.open.then(|| sections(app, &card.body));
    let progress = state(app, &card.more, id, false);
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
            id,
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
            id,
        ))
        .children(said.map(|said| view().px(px(12.0)).py(px(8.0)).child(said)))
        .children(data.map(|data| view().px(px(12.0)).pb(px(10.0)).child(data)))
        .children(progress.map(|progress| view().px(px(12.0)).pb(px(8.0)).child(progress)))
        .into_element()
}

/// The turn's foot: model, an unusual stop and usage, quiet; pressing it shows every fact the
/// turn recorded.
pub(super) fn footer(app: AppState, footer: &Footer, id: &str) -> AnyElement {
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
        .test_id(&format!("item-{id}"))
        .min_w(px(0.0))
        .when(form != Form::Terminal, |block| block.mt(px(2.0)))
        .child(pressable(
            app,
            row,
            footer.action.as_ref(),
            footer.words.clone(),
            footer.open,
            id,
        ))
        .when(footer.open, |block| {
            block.child(indented(app, sections(app, &footer.body)))
        })
        .children(state(app, &footer.more, id, false))
        .into_element()
}

/// Copy reply under a finished agent message (`btn-copy-reply-<id>`): its words whole, as
/// written, for pasting into another session; where some block is only a preview, its words
/// and receipt say the copy is partial.
pub(in crate::screens::agents) fn copy_reply(
    app: AppState,
    partial: bool,
    id: &str,
    reply: impl Fn() -> Option<Reply> + 'static,
) -> AnyElement {
    let copy = crate::ui::composer::copier(app);
    view()
        .flex_row()
        .when(app.form != Form::Terminal, |row| row.mt(px(2.0)))
        .child(button_with(
            app,
            Reply::label(partial),
            ButtonKind::Quiet,
            ButtonOptions {
                compact: true,
                ..ButtonOptions::default()
            },
            format!("btn-copy-reply-{id}"),
            move || {
                if let Some(reply) = reply() {
                    copy(&reply.text);
                    app.toast(reply.receipt());
                }
            },
        ))
        .into_element()
}
