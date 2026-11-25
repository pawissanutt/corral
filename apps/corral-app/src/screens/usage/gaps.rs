//! The period at a glance: the four tiles (drawn as Home's), then what the totals leave out in
//! one quiet line with Details, which opens every reason in full, each partial group's reasons
//! and the notes on where the figures come from. test ids "usage-stats", "stat-usage-<key>",
//! "usage-coverage", "btn-usage-details", "usage-details".

use nsh_core::prelude::*;

use crate::model::usage::{DETAILS, Gaps, HIDE_DETAILS, Page, State};
use crate::model::{Form, Hue};
use crate::screens::home::tile;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::text::{h3, mono};
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::controls;

/// The tiles in a row that wraps; none until corrald has answered for the period.
pub(super) fn stats(app: AppState, page: &Page) -> Option<AnyElement> {
    if page.stats.is_empty() {
        return None;
    }
    let row = view()
        .test_id("usage-stats")
        .flex_row()
        .flex_wrap()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 10.0 }))
        .min_w(px(0.0))
        .children(
            page.stats
                .iter()
                .cloned()
                .map(|stat| tile(app, stat))
                .collect::<Vec<_>>(),
        );
    Some(row.into_element())
}

/// Words that wrap, in `ink`, at `size` points off the terminal.
fn words(app: AppState, content: &str, ink: Token, size: f32) -> Text {
    let line = text(content.to_owned())
        .min_w(px(0.0))
        .text_color(color(ink));
    match app.form {
        Form::Terminal => line,
        Form::Desktop | Form::Phone => line.font_family(FONT_TEXT).text_size(px(size)),
    }
}

/// The coverage line with its Details toggle, and the details while they are open.
pub(super) fn strip(app: AppState, gaps: &Gaps) -> AnyElement {
    let label = if gaps.open { HIDE_DETAILS } else { DETAILS };
    let toggle = button(
        app,
        label,
        ButtonKind::Ghost,
        "btn-usage-details",
        move || controls::change(app, State::toggle_details),
    );
    let line = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .min_w(px(0.0))
        .children(gaps.partial.then(|| {
            text("◐")
                .flex_shrink_0()
                .text_color(hue_ink(Hue::Held))
        }))
        .child(
            words(app, &gaps.line, Token::Ink2, 12.5)
                .flex_shrink(1.0)
                .flex_grow(1.0),
        )
        .child(toggle);
    view()
        .test_id("usage-coverage")
        .child(line)
        .children(gaps.open.then(|| details(app, gaps)))
        .into_element()
}

/// Every reason in full, each partial group's reasons, then the notes.
fn details(app: AppState, gaps: &Gaps) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let groups = (!gaps.groups.is_empty()).then(|| {
        view()
            .gap(px(if terminal { 0.0 } else { 4.0 }))
            .child(h3(app, "By group", None))
            .children(
                gaps.groups
                    .iter()
                    .map(|(key, why)| {
                        view()
                            .flex_row()
                            .items_baseline()
                            .gap(px(8.0))
                            .min_w(px(0.0))
                            .child(mono(app, key.clone()).number_of_lines(1u32).flex_shrink_0())
                            .child(words(app, why, Token::Ink3, 12.0).flex_shrink(1.0))
                            .into_element()
                    })
                    .collect::<Vec<_>>(),
            )
            .into_element()
    });
    view()
        .test_id("usage-details")
        .gap(px(if terminal { 0.0 } else { 6.0 }))
        .pt(px(if terminal { 0.0 } else { 6.0 }))
        .children(
            gaps.reasons
                .iter()
                .map(|reason| words(app, reason, Token::Ink2, 12.5).into_element())
                .collect::<Vec<_>>(),
        )
        .children(groups)
        .children(
            gaps.notes
                .iter()
                .map(|note| words(app, note, Token::Ink3, 11.5).into_element())
                .collect::<Vec<_>>(),
        )
        .into_element()
}
