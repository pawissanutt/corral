//! The status line under the composer (`session::StatusBar`): the role pill, the session's cost
//! (`$–` while unknown, `+` when a member's is), the context meter in its tone from 80 % and
//! 95 % (`ctx –` without a window), the model and effort, the quiet time and the queued count
//! where reported, and the host's standing alerts and last error in the held colour, wrapping,
//! never dropped. Values received before corrald became unreachable are marked stale. Desktop:
//! one 26 pt row of quiet 11.5 pt words split by hairlines; terminal: one row of Cells split by
//! `│`. The phone's page has none; Ask's sheet keeps the short pill and meter.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{SessionPage, StatusBar};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::body::conversation;

/// `measured`: its words keep to the transcript's measure (the page), its rule edge to edge.
pub(super) fn bar(app: AppState, page: Memo<SessionPage>, measured: bool) -> AnyElement {
    let status = computed(move || page.with(|p| p.status.clone()));
    dynamic(move || status.with(|status| untracked(|| face(app, status, measured))))
}

fn face(app: AppState, status: &StatusBar, measured: bool) -> AnyElement {
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
    if !phone {
        parts.extend(
            [&status.model_effort, &status.quiet, &status.queued]
                .into_iter()
                .flatten()
                .map(|fact| words(fact.clone(), quiet, true)),
        );
    }
    if status.stale {
        parts.push(words("stale".into(), hue_ink(Hue::Held), false));
    }
    let mut row = Vec::with_capacity(parts.len() * 2);
    for (at, part) in parts.into_iter().enumerate() {
        if at > 0 {
            row.push(separator(form));
        }
        row.push(view().flex_shrink_0().child(part).into_element());
    }
    // The facts keep one row, their tail clipped where the page is narrow, so the conversation
    // keeps its room; the alerts wrap under them and are never dropped.
    let facts = view()
        .flex_row()
        .items_center()
        .overflow_hidden()
        .gap(px(8.0))
        .children(row);
    let alerts: Vec<AnyElement> = status
        .alerts
        .iter()
        .map(|alert| words(alert.text.clone(), hue_ink(alert.hue), false))
        .collect();
    let line = view()
        .test_id("session-status")
        .child(facts)
        .when(!alerts.is_empty(), |line| {
            line.child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .children(alerts),
            )
        });
    match form {
        Form::Terminal => view()
            .flex_none()
            .px(term::cells(1.0))
            .child(line)
            .into_element(),
        Form::Desktop => view()
            .flex_none()
            .border_t(px(1.0))
            .border_color(color(Token::Rule))
            .child(conversation(
                app,
                measured,
                line.min_h(px(26.0)).px(px(18.0)).py(px(4.0)).into_element(),
            ))
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
