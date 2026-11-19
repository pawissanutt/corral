//! Messages' words and thinking in a watched session's transcript: rendered Markdown at the
//! desktop's measure, and thinking muted under its lead, folded to one row.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::session::transcript::{Prose, Thinking};
use crate::screens::markdown::markdown;
use crate::state::AppState;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::PARAGRAPH;
use super::parts::{disclosure, pressable, small, state};

/// One blink of the streaming caret, as the assistant thread's.
const BLINK_MS: f32 = 1000.0;

/// The streaming caret (`.caret`): a 7 by 14 pt block of ink, blinking; `▌` in a terminal.
pub(super) fn caret(app: AppState) -> AnyElement {
    if app.form == Form::Terminal {
        return text("▌")
            .text_color(color(Token::Ink))
            .accessibility_elements_hidden(true)
            .into_element();
    }
    let blink = motion().entering(
        MotionSpec::new(KeyframeTiming::new(BLINK_MS).infinite())
            .values(MotionChannel::Opacity, [1.0, 0.0, 1.0]),
    );
    view()
        .w(px(7.0))
        .h(px(14.0))
        .bg(color(Token::Ink))
        .accessibility_elements_hidden(true)
        .bind_motion(&blink)
        .into_element()
}

/// Words of a message: rendered Markdown at most ~70 characters wide on the desktop, then
/// where the rest of a continued item stands.
pub(super) fn prose(app: AppState, words: &Prose, n: usize) -> AnyElement {
    view()
        .min_w(px(0.0))
        .when(app.form == Form::Desktop, |block| {
            block.max_w(px(PARAGRAPH))
        })
        .when(app.form != Form::Terminal, |block| block.mb(px(6.0)))
        .child(markdown(app, &words.markdown, words.streaming))
        .children(state(app, &words.more, n, true))
        .into_element()
}

/// Thinking: a `thinking` lead, then the reasoning muted so it never reads as the answer;
/// folded, one row says what it hides. Pressing the lead folds or shows it.
pub(super) fn thinking(app: AppState, thought: &Thinking, n: usize) -> AnyElement {
    let form = app.form;
    let lead = view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .when(form != Form::Terminal, |row| {
            row.py(px(2.0)).rounded(px(4.0))
        })
        .child(
            small(app, "thinking", Token::Ink3).font_weight(if form == Form::Terminal {
                FontWeight::BOLD
            } else {
                FontWeight::SEMIBOLD
            }),
        )
        .child(disclosure(app, thought.open))
        .when(!thought.open, |row| {
            row.child(small(app, thought.hidden.clone(), Token::Ink3))
        })
        .when(thought.streaming && !thought.open, |row| {
            row.child(caret(app))
        });
    let lead = pressable(
        app,
        lead,
        Some(&thought.action),
        "Thinking".into(),
        thought.open,
        n,
    );
    let reasoning = thought.open.then(|| {
        let words = text(thought.text.clone()).text_color(color(Token::Ink3));
        let words = if form == Form::Terminal {
            words
        } else {
            words
                .font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 14.0 } else { 12.5 }))
                .line_height(px(if form == Form::Phone { 20.0 } else { 18.0 }))
        };
        view()
            .min_w(px(0.0))
            .when(form == Form::Desktop, |block| block.max_w(px(PARAGRAPH)))
            .child(words)
            .when(thought.streaming, |block| block.child(caret(app)))
    });
    view()
        .min_w(px(0.0))
        .when(form != Form::Terminal, |block| block.mb(px(6.0)))
        .child(lead)
        .children(reasoning)
        .children(state(app, &thought.more, n, true))
        .into_element()
}
