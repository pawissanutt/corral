//! The status bar under the composer (`session::StatusBar`): the role pill, the session's cost
//! (`$–` while unknown, `+` when a member's is), the context meter in its tone from 80 % and
//! 95 % (`ctx –` without a window), and the host's standing alerts and last error in the held
//! colour, wrapping, never dropped. Values received before corrald became unreachable are marked
//! stale. Desktop: one 26 pt row of quiet 11.5 pt words split by hairlines; phone: the short
//! pill and meter in one compact row; terminal: one row of Cells split by `│`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{SessionPage, StatusBar};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

pub(super) fn bar(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let status = computed(move || page.with(|p| p.status.clone()));
    dynamic(move || status.with(|status| untracked(|| face(app, status))))
}

fn face(app: AppState, status: &StatusBar) -> AnyElement {
    let form = app.form;
    let phone = form == Form::Phone;
    let words = |content: String, ink: AdaptiveColor, mono: bool| {
        let words = text(content).number_of_lines(1u32).text_color(ink);
        match form {
            Form::Terminal => words.into_element(),
            _ => words
                .font_family(if mono { FONT_MONO } else { FONT_TEXT })
                .text_size(px(if phone { 12.0 } else { 11.5 }))
                .into_element(),
        }
    };
    let quiet = color(if form == Form::Terminal {
        Token::Ink
    } else {
        Token::Ink2
    });
    let pill = text(if phone {
        status.role.short.clone()
    } else {
        status.role.full.clone()
    })
    .number_of_lines(1u32)
    .font_weight(FontWeight::BOLD)
    .text_color(color(Token::Ink));
    let pill = match form {
        Form::Terminal => pill.into_element(),
        _ => view()
            .px(px(5.0))
            .rounded(px(3.0))
            .bg(color(Token::Surface2))
            .child(
                pill.font_family(FONT_TEXT)
                    .text_size(px(10.5))
                    .letter_spacing(px(0.4)),
            )
            .into_element(),
    };
    let meter_ink = if status.meter.hue == Hue::Neutral {
        quiet
    } else {
        hue_ink(status.meter.hue)
    };
    let mut parts = vec![
        pill,
        words(status.cost.text.clone(), quiet, true),
        words(
            if phone {
                status.meter.short.clone()
            } else {
                status.meter.full.clone()
            },
            meter_ink,
            true,
        ),
    ];
    if status.stale {
        parts.push(words("stale".into(), hue_ink(Hue::Held), false));
    }
    parts.extend(
        status
            .alerts
            .iter()
            .map(|alert| words(alert.text.clone(), hue_ink(alert.hue), false)),
    );
    let mut row = Vec::with_capacity(parts.len() * 2);
    for (at, part) in parts.into_iter().enumerate() {
        if at > 0 {
            row.push(separator(form));
        }
        row.push(part);
    }
    let line = view()
        .test_id("session-status")
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .children(row);
    match form {
        Form::Terminal => view()
            .flex_none()
            .px(term::cells(1.0))
            .child(line)
            .into_element(),
        Form::Desktop => line
            .flex_none()
            .min_h(px(26.0))
            .px(px(18.0))
            .py(px(4.0))
            .border_t(px(1.0))
            .border_color(color(Token::Rule))
            .into_element(),
        Form::Phone => line
            .flex_none()
            .px(px(16.0))
            .pt(px(2.0))
            .pb(px(6.0))
            .into_element(),
    }
}

/// The hairline between two parts (`│` in a terminal).
fn separator(form: Form) -> AnyElement {
    match form {
        Form::Terminal => text("│").text_color(color(Token::Rule2)).into_element(),
        _ => view()
            .w(px(1.0))
            .h(px(12.0))
            .bg(color(Token::Rule2))
            .accessibility_elements_hidden(true)
            .into_element(),
    }
}
