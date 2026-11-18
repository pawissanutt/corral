//! The Onto line (`field-onto`): the branch approving lands onto, the run's base until another
//! is typed, the project's local branches offered under it one press each (`seg-onto-<branch>`),
//! and what approving does to the candidate. Desktop and terminal pin it over the decide bar;
//! the phone keeps it in the page beside Run gate and Start review. Return in the field
//! approves. The words live in the model (`Action::Onto`), and the field is built once per
//! shown candidate, so typing re-reads the model without rebuilding the field.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::projects::BRANCHES_CAPTION;
use crate::model::review::{Action, Target};
use crate::model::text::Rich;
use crate::ui::kit::hint::hinted;
use crate::ui::kit::suggest::suggestions;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::{Ctx, act, rich_at};

/// The field's width beside its caption: room for `release/2026.11` and a little more.
const FIELD: f32 = 190.0;
const FIELD_CELLS: f32 = 22.0;
const TARGET: f32 = 44.0;

/// The line, or nothing while no candidate shows.
pub(super) fn onto(cx: &Ctx) -> AnyElement {
    let page = cx.page;
    let candidate = computed(move || {
        page.with(|page| {
            page.detail
                .as_ref()
                .map(|detail| (detail.header.task.clone(), detail.decide.target.clone()))
        })
    });
    // Keyed by the candidate: another candidate brings its own base and hint; typing does not.
    let shown = computed(move || candidate.with(|shown| shown.as_ref().map(|(id, ..)| id.clone())));
    let cx = cx.clone();
    dynamic(move || {
        shown.get()?;
        let target = untracked(|| candidate.get().map(|(_, target)| target))?;
        Some(untracked(|| line(&cx, &target)))
    })
    .into_element()
}

fn line(cx: &Ctx, target: &Target) -> AnyElement {
    let app = cx.app;
    let form = app.form;
    let page = cx.page;
    let value = computed(move || {
        page.with(|page| {
            page.detail
                .as_ref()
                .map(|detail| detail.decide.target.value.clone())
                .unwrap_or_default()
        })
    });
    let typed = {
        let cx = cx.clone();
        move |words: &str| act(&cx, Action::Onto(words.to_owned()))()
    };
    let approve = act(cx, Action::Approve);
    let field = input()
        .test_id("field-onto")
        .label(target.label)
        .placeholder(target.placeholder)
        .value(move || value.get())
        .on_change(typed)
        .on_submit(move |_: &str| approve())
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)));
    let field = match form {
        Form::Terminal => field
            .h(term::rows(1.0))
            .w(term::cells(FIELD_CELLS))
            .flex_shrink_0()
            .px(term::cells(1.0))
            .bg(color(Token::Surface))
            .focus(|s| s.bg(color(Token::Sel))),
        Form::Desktop => field
            .w(px(FIELD))
            .flex_shrink_0()
            .h(px(28.0))
            .px(px(10.0))
            .font_family(FONT_TEXT)
            .text_size(px(13.0))
            .rounded(px(6.0))
            .bg(AdaptiveColor::TRANSPARENT)
            .border_1()
            .border_color(color(Token::Rule2))
            .focus(|s| s.border_color(color(Token::Focus))),
        Form::Phone => field
            .flex_1()
            .min_h(px(TARGET))
            .px(px(12.0))
            .font_family(FONT_TEXT)
            .text_size(px(16.0))
            .rounded(px(10.0))
            .bg(AdaptiveColor::TRANSPARENT)
            .border_1()
            .border_color(color(Token::Rule2))
            .focus(|s| s.border_color(color(Token::Focus))),
    };
    let caption = text(target.label)
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    let caption = match form {
        Form::Terminal => caption,
        Form::Desktop => caption
            .font_family(FONT_TEXT)
            .text_size(px(12.0))
            .font_weight(FontWeight::SEMIBOLD),
        Form::Phone => caption
            .font_family(FONT_TEXT)
            .text_size(px(14.0))
            .font_weight(FontWeight::SEMIBOLD),
    };
    let caption = hinted(app, caption, target.hint.clone());
    // What approving does follows the field and rewords as the branch changes; the phone says
    // it in the confirm sheet instead.
    let explanation = computed(move || {
        page.with(|page| {
            page.detail
                .as_ref()
                .and_then(|detail| detail.decide.explanation.clone())
        })
    });
    let said = dynamic(move || {
        let words: Option<Rich> = explanation.get();
        words.map(|words| {
            rich_at(app, &words, 12.0, Token::Ink3, false)
                .number_of_lines(1u32)
                .truncate()
                .flex_1()
                .min_w(px(0.0))
        })
    });
    let branches = computed(move || {
        page.with(|page| {
            page.detail
                .as_ref()
                .map(|detail| detail.decide.target.branches.clone())
                .unwrap_or_default()
        })
    });
    let pick = {
        let cx = cx.clone();
        move |branch: String| act(&cx, Action::Onto(branch))()
    };
    let offered = suggestions(app, BRANCHES_CAPTION, "onto", branches, pick);
    let row = view().flex_row().items_center().min_w(px(0.0));
    match form {
        Form::Terminal => view()
            .flex_none()
            .child(term::hrule())
            .child(
                row.px(term::cells(1.0))
                    .child(caption)
                    .child(term::space(1.0))
                    .child(field)
                    .child(term::space(2.0))
                    .child(view().flex_1().min_w(px(0.0)).child(said)),
            )
            .child(view().px(term::cells(1.0)).child(offered))
            .into_element(),
        Form::Desktop => view()
            .flex_none()
            .gap(px(6.0))
            .px(px(24.0))
            .pt(px(10.0))
            .bg(color(Token::Bg))
            .border_t(px(1.0))
            .border_color(color(Token::Rule))
            .child(
                row.gap(px(10.0))
                    .child(caption)
                    .child(field)
                    .child(view().flex_1().min_w(px(0.0)).child(said)),
            )
            .child(offered)
            .into_element(),
        Form::Phone => view()
            .gap(px(8.0))
            .px(px(18.0))
            .pt(px(14.0))
            .child(row.gap(px(10.0)).child(caption).child(field))
            .child(offered)
            .into_element(),
    }
}
