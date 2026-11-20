//! What every part's body shares, in Details and in the part's full view: its scrolling
//! surface, and the frame before its content (`session::PartBody`): its summary in its tone, why
//! the host cannot serve it now, and where its read stands (reading, failed with Retry, does not
//! apply, read at a time with Reread).

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{PartBody, ReadState};
use crate::model::text::span;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::notice::{empty, error, loading, notice};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::super::run;
use super::hue_words;

/// A part's scrolling surface, `test_id` on it, inset as the form's pages are.
pub(super) fn surface(app: AppState, test_id: &str, content: AnyElement) -> AnyElement {
    let surface = scroll().test_id(test_id).flex_1().min_h(px(0.0));
    match app.form {
        Form::Terminal => surface
            .child(view().px(term::cells(1.0)).child(content))
            .into_element(),
        Form::Desktop => surface
            .child(view().px(px(18.0)).py(px(12.0)).child(content))
            .into_element(),
        Form::Phone => surface
            .child(view().px(px(16.0)).py(px(10.0)).child(content))
            .into_element(),
    }
}

/// The body of a tab or panel before its content: the summary in its tone; why the host cannot
/// serve it now; then where its read stands: the reading skeleton, the failure with Retry, why it
/// does not apply, or when it was read with Reread. `slug` names its Reread (`btn-reread-<slug>`).
pub(super) fn part(app: AppState, body: &PartBody, slug: &str) -> AnyElement {
    let form = app.form;
    let reread = body.reread.clone().map(|action| {
        let label = if matches!(body.read, ReadState::Failed(_)) {
            "Retry"
        } else {
            "Reread"
        };
        button(
            app,
            label,
            ButtonKind::Quiet,
            format!("btn-reread-{slug}"),
            move || run(app, action.clone()),
        )
    });
    let summary = text(body.summary.clone())
        .test_id(&format!("part-{slug}-summary"))
        .text_color(hue_words(form, body.hue));
    let summary = match form {
        Form::Terminal => summary.font_weight(FontWeight::BOLD),
        _ => summary
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 15.0 } else { 14.0 }))
            .font_weight(FontWeight::SEMIBOLD),
    };
    let unavailable = body.unavailable.clone().map(|why| {
        notice(
            app,
            Hue::Held,
            &vec![span(format!("unavailable: {why}"))],
            None,
        )
    });
    let read = match &body.read {
        ReadState::NotRead | ReadState::Reading if body.unavailable.is_none() => Some(loading(
            app,
            body.read.words(),
            &[(0.5, 10.0), (0.8, 10.0), (0.65, 10.0)],
        )),
        ReadState::NotRead | ReadState::Reading => None,
        ReadState::Failed(_) => Some(error(app, &vec![span(body.read.words())], reread)),
        ReadState::DoesNotApply(why) => Some(empty(app, "Does not apply", why.clone(), None)),
        ReadState::ReadingKept | ReadState::Read { .. } | ReadState::Stale { .. } => {
            let words = text(body.read.words())
                .number_of_lines(1u32)
                .truncate()
                .min_w(px(0.0))
                .text_color(if body.read.hue() == Hue::Neutral {
                    color(Token::Ink3)
                } else {
                    hue_ink(body.read.hue())
                });
            let words = match form {
                Form::Terminal => words,
                _ => words.font_family(FONT_TEXT).text_size(px(12.0)),
            };
            Some(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(if form == Form::Terminal { 16.0 } else { 10.0 }))
                    .child(words)
                    .children(reread)
                    .into_element(),
            )
        }
    };
    view()
        .test_id(&format!("part-{slug}"))
        .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .child(summary)
        .children(unavailable)
        .children(read)
        .into_element()
}
