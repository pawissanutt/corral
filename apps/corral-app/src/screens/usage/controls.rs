//! The Usage page's controls: the period (7 days, 30 days, this month, a custom range) and the
//! grouping switches, and, while Custom is chosen, the From and To fields with Apply (Return in
//! either field applies too) and why a typed range was not applied. The fields are built once
//! while Custom stays chosen, so typing re-reads the model without rebuilding them.

use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use crate::model::usage::{self, GROUPINGS, PERIODS, Page, State};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::kit::button::{ButtonKind, button, seg};
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// A date field: room for `2026-11-18` and its padding.
const FIELD: f32 = 116.0;
const FIELD_CELLS: f32 = 14.0;
/// The phone's least touch target.
const TARGET: f32 = 44.0;

/// Runs `change` on the page's selection state; the switches, fields and the Details toggle
/// cause no effects.
pub(super) fn change(app: AppState, change: impl FnOnce(&mut State) + 'static) {
    app.act(app.usage, |state, _| {
        change(state);
        Vec::new()
    });
}

/// 7 days, 30 days, This month, Custom: the period from `app.usage`.
pub(super) fn period_seg(app: AppState) -> AnyElement {
    seg(
        app,
        &PERIODS,
        move || app.usage.with(|state| state.period),
        move |period| {
            let today = app.feed.with_untracked(|feed| usage::local(feed).today);
            change(app, move |state| state.select_period(period, today));
        },
        "seg-period",
    )
}

/// Profile, Model, Task, Day: the grouping from `app.usage`.
pub(super) fn grouping_seg(app: AppState) -> AnyElement {
    seg(
        app,
        &GROUPINGS,
        move || app.usage.with(|state| state.by),
        move |by| change(app, move |state| state.select_by(by)),
        "seg-by",
    )
}

/// The terminal's and the phone's controls at the top of the scroll: the two switches (side by
/// side in a terminal while they fit, one under the other on the phone), then the custom range.
pub(super) fn stacked(app: AppState, data: Memo<Page>) -> AnyElement {
    let switches = view().child(period_seg(app)).child(grouping_seg(app));
    let switches = match app.form {
        Form::Phone => switches.gap(px(8.0)),
        Form::Desktop | Form::Terminal => switches
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_x(px(24.0)),
    };
    let row = view().child(switches).child(custom_row(app, data));
    match app.form {
        Form::Terminal => row.into_element(),
        Form::Desktop | Form::Phone => row.px(px(16.0)).pt(px(4.0)).pb(px(4.0)).into_element(),
    }
}

/// While Custom is chosen: From and To, Apply, the hint, and why a typed range was refused.
pub(super) fn custom_row(app: AppState, data: Memo<Page>) -> AnyElement {
    let open = computed(move || data.with(|page| page.custom.is_some()));
    dynamic(move || open.get().then(|| untracked(|| fields(app, data))))
}

fn field(
    app: AppState,
    (test_id, label, placeholder): (&'static str, &'static str, &'static str),
    value: impl Fn(&State) -> String + Copy + 'static,
    typed: impl Fn(&mut State, &str) + Copy + 'static,
) -> AnyElement {
    let form = app.form;
    let handle = element_ref();
    let entry = copyable_input(app, Some(handle), move || app.usage.with(value))
        .test_id(test_id)
        .label(label)
        .placeholder(placeholder)
        .on_change(move |words: &str| {
            let words = words.to_owned();
            change(app, move |state| typed(state, &words));
        })
        // Return applies the range and leaves the keyboard in the field, so a refused range
        // can be corrected and the other date typed without finding the field again.
        .on_submit(move |_: &str| {
            change(app, State::apply_custom);
            after_answer(move || handle.focus());
        })
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)));
    let entry = match form {
        Form::Terminal => entry
            .h(term::rows(1.0))
            .w(term::cells(FIELD_CELLS))
            .flex_shrink_0()
            .px(term::cells(1.0))
            .bg(color(Token::Surface))
            .focus(|s| s.bg(color(Token::Sel))),
        Form::Desktop => entry
            .w(px(FIELD))
            .flex_shrink_0()
            .h(px(28.0))
            .px(px(10.0))
            .font_family(FONT_MONO)
            .text_size(px(12.5))
            .tabular_nums()
            .rounded(px(6.0))
            .bg(AdaptiveColor::TRANSPARENT)
            .border_1()
            .border_color(color(Token::Rule2))
            .focus(|s| s.border_color(color(Token::Focus))),
        Form::Phone => entry
            .flex_1()
            .min_h(px(TARGET))
            .px(px(12.0))
            .font_family(FONT_MONO)
            .text_size(px(16.0))
            .rounded(px(10.0))
            .bg(AdaptiveColor::TRANSPARENT)
            .border_1()
            .border_color(color(Token::Rule2))
            .focus(|s| s.border_color(color(Token::Focus))),
    };
    let caption = text(label)
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    let caption = match form {
        Form::Terminal => caption,
        Form::Desktop => caption.font_family(FONT_TEXT).text_size(px(12.0)),
        Form::Phone => caption.font_family(FONT_TEXT).text_size(px(14.0)),
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 8.0 } else { 6.0 }))
        .when(form == Form::Phone, |row| row.flex_1().min_w(px(0.0)))
        .child(caption)
        .child(entry)
        .into_element()
}

fn fields(app: AppState, data: Memo<Page>) -> AnyElement {
    let form = app.form;
    let refused = computed(move || {
        data.with(|page| {
            page.custom
                .as_ref()
                .and_then(|custom| custom.refused.clone())
        })
    });
    let Some(custom) = untracked(|| data.with(|page| page.custom.clone())) else {
        return view().into_element();
    };
    let from = field(
        app,
        ("field-usage-from", custom.from, custom.placeholder),
        |state| state.from.clone(),
        State::type_from,
    );
    let to = field(
        app,
        ("field-usage-to", custom.to, custom.placeholder),
        |state| state.to.clone(),
        State::type_to,
    );
    let (apply, hint) = (custom.apply, custom.hint);
    let apply = button(
        app,
        apply,
        ButtonKind::Ghost,
        "btn-usage-apply",
        move || change(app, State::apply_custom),
    );
    let quiet = |words: String| {
        let line = text(words).text_color(color(Token::Ink3));
        match form {
            Form::Terminal => line.into_element(),
            Form::Desktop | Form::Phone => line
                .font_family(FONT_TEXT)
                .text_size(px(11.5))
                .into_element(),
        }
    };
    let refused = dynamic(move || {
        refused.get().map(|why| {
            let line = text(why)
                .test_id("usage-range-refused")
                .text_color(hue_ink(Hue::Fail));
            match form {
                Form::Terminal => line.into_element(),
                Form::Desktop | Form::Phone => line
                    .font_family(FONT_TEXT)
                    .text_size(px(12.0))
                    .into_element(),
            }
        })
    });
    let row = match form {
        Form::Phone => view()
            .gap(px(8.0))
            .child(view().flex_row().gap(px(8.0)).child(from).child(to))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .child(apply)
                    .child(quiet(hint)),
            ),
        Form::Desktop | Form::Terminal => view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_x(px(if form == Form::Terminal { 16.0 } else { 12.0 }))
            .gap_y(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
            .child(from)
            .child(to)
            .child(apply)
            .child(quiet(hint)),
    };
    let block = view()
        .test_id("usage-range")
        .role(AccessibilityRole::Group)
        .label("Custom period")
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(row)
        .child(refused);
    match form {
        Form::Terminal => block.into_element(),
        Form::Phone => block.pt(px(10.0)).pb(px(4.0)).into_element(),
        Form::Desktop => block.px(px(18.0)).pt(px(12.0)).pb(px(4.0)).into_element(),
    }
}
