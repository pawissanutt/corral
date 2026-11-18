//! The Brief's Rounds block: its heading with the task's round against its limit, the reviewer
//! session that stays with the task (with the press that opens it), then each round oldest
//! first, its title over the checks that judged it in the readiness chain's form, the
//! reviewer's notes quoted.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::review::{ReviewerLine, Round, Rounds};
use crate::ui::kit::hint::hinted;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::brief::{caption, chain};
use super::{Ctx, command};

pub(super) fn rounds(cx: &Ctx, rounds: &Rounds) -> AnyElement {
    let app = cx.app;
    let heading = hinted(
        app,
        view().child(caption(app, rounds.heading, rounds.count.clone(), false)),
        rounds.hint,
    );
    view()
        .test_id("review-rounds")
        .child(heading)
        .children(rounds.reviewer.as_ref().map(|line| reviewer(cx, line)))
        .children(
            rounds
                .rounds
                .iter()
                .map(|round| one(cx, round))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// The live reviewer: who it is and since when, its hint saying why it stays, then Open.
fn reviewer(cx: &Ctx, line: &ReviewerLine) -> AnyElement {
    let app = cx.app;
    let form = app.form;
    let words = text(line.text.clone())
        .number_of_lines(1u32)
        .truncate()
        .min_w(px(0.0))
        .text_color(color(Token::Ink2));
    let words = match form {
        Form::Terminal => words,
        Form::Desktop | Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 14.0 } else { 13.0 })),
    };
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_x(if form == Form::Terminal {
            term::cells(2.0)
        } else {
            px(10.0)
        })
        .gap_y(px(4.0))
        .when(form != Form::Terminal, |row| row.mb(px(12.0)))
        .child(
            view()
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .child(hinted(app, words, line.hint.clone())),
        )
        .child(command(cx, &line.open))
        .into_element()
}

/// One round: its title, bold while it is the current one, then its checks.
fn one(cx: &Ctx, round: &Round) -> AnyElement {
    let app = cx.app;
    let form = app.form;
    let title = text(round.title.clone())
        .role(AccessibilityRole::Header)
        .number_of_lines(1u32)
        .font_weight(if round.current {
            FontWeight::BOLD
        } else {
            FontWeight::SEMIBOLD
        })
        .text_color(color(if round.current {
            Token::Ink
        } else {
            Token::Ink2
        }));
    let title = match form {
        Form::Terminal => title,
        Form::Desktop | Form::Phone => title
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 13.5 } else { 12.5 }))
            .mb(px(8.0)),
    };
    view()
        .when(form != Form::Terminal, |round| round.mb(px(14.0)))
        .child(title)
        .child(chain(app, &round.links))
        .into_element()
}
