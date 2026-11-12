//! Recommendation: the answer options as radio cards, the alternate answer and the worker's
//! reason.

use nsh_core::Checked;
use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use super::{act, code, press, rich_at, ring, words};
use crate::model::inbox::{Action, AnswerOption, Recommendation};
use crate::model::{Form, Hue, Rich, span};
use crate::state::AppState;
use crate::ui::kit::chip::chip;
use crate::ui::kit::text::label_value;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The options as radio cards, the alternate answer, the worker's reason.
pub(super) fn recommendation(app: AppState, part: Recommendation) -> AnyElement {
    let form = app.form;
    let reason: Rich = vec![
        span(part.reason_label).bold(),
        span(" "),
        span(part.reason).dim(),
    ];
    view()
        .child(
            view()
                .role(AccessibilityRole::Radiogroup)
                .label(part.label)
                .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
                .children(part.options.iter().map(|option| {
                    answer_option(app, &part.consequence_labels, part.locked, option)
                }))
                .child(draft_row(app, &part)),
        )
        .child(
            view()
                .when(form != Form::Terminal, |v| v.mt(px(10.0)))
                .when(form == Form::Terminal, |v| v.child(term::blank()))
                .child(rich_at(
                    app,
                    &reason,
                    if form == Form::Phone { 13.5 } else { 12.0 },
                    color(Token::Ink2),
                )),
        )
        .into_element()
}

/// One option (`.opt`): its number in a ring (filled ink when chosen), the label with the
/// worker's recommendation tag, then Then, Unblocks and Costs (three columns on desktop, one
/// under another on the phone and in a terminal), the proof link switching the evidence tab.
/// Chosen: a 1.5 pt ink edge on the surface. Locked: the others fade to half.
fn answer_option(
    app: AppState,
    labels: &[&'static str; 3],
    locked: bool,
    option: &AnswerOption,
) -> AnyElement {
    let form = app.form;
    let on = option.selected;
    let value_size = if form == Form::Phone { 14.0 } else { 12.5 };
    let cells: Vec<AnyElement> = labels
        .iter()
        .zip(option.consequences)
        .enumerate()
        .map(|(at, (label, value))| {
            let dd = words(app, value, value_size, Token::Ink2)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .line_height(px(value_size * 1.5))
                .when(at == labels.len() - 1, |dd| {
                    dd.child(text(" ")).child(proof_link(app, option))
                });
            consequence(app, *label, dd)
        })
        .collect();
    let consequences = match form {
        Form::Desktop => view()
            .flex_row()
            .items_start()
            .gap(px(18.0))
            .mt(px(8.0))
            .children(cells),
        Form::Phone | Form::Terminal => view().children(cells),
    };
    // The label wraps in the room the card leaves it: on desktop the tag sits at its end, on the
    // phone and in a terminal under it (`.ol`, `.rec`).
    let label = words(
        app,
        option.label,
        if form == Form::Phone { 15.5 } else { 14.0 },
        Token::Ink,
    )
    .flex_shrink(1.0)
    .min_w(px(0.0))
    .font_weight(if form == Form::Terminal {
        FontWeight::BOLD
    } else {
        FontWeight::SEMIBOLD
    })
    .line_height(px(if form == Form::Phone { 21.7 } else { 19.6 }));
    let tag = option.recommendation_chip.map(|tag| {
        view()
            .flex_row()
            .flex_shrink_0()
            .child(recommends(app, tag))
    });
    let title = match form {
        Form::Desktop => view()
            .flex_row()
            .items_start()
            .gap(px(8.0))
            .child(label)
            .children(tag.map(|tag| tag.mt(px(1.0)))),
        Form::Phone | Form::Terminal => view()
            .child(label)
            .children(tag.map(|tag| tag.self_start().mt(px(6.0)))),
    };
    let card = view()
        .flex_row()
        .items_start()
        .min_w(px(0.0))
        .role(AccessibilityRole::Radio)
        .label(option.label)
        .accessibility_state(AccessibilityState {
            checked: Some(if on { Checked::True } else { Checked::False }),
            disabled: !option.enabled,
            ..AccessibilityState::default()
        })
        .test_id(&format!("opt-{}", option.number))
        .opacity(if locked && !on { 0.5 } else { 1.0 })
        .when(option.enabled, |v| {
            v.cursor_pointer()
                .on_press(press(app, option.action.clone()))
                .focus_visible(ring)
        })
        .child(number(app, option.number.to_string(), on))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(title)
                .child(consequences),
        );
    match form {
        Form::Terminal => card.gap(px(8.0)).pb(px(16.0)).into_element(),
        Form::Desktop | Form::Phone => {
            let phone = form == Form::Phone;
            card.gap(px(10.0))
                .pl(px(if phone { 12.0 } else { 10.0 }))
                .pr(px(14.0))
                .py(px(if phone { 14.0 } else { 12.0 }))
                .rounded(px(if phone { 12.0 } else { 8.0 }))
                .border(px(if on { 1.5 } else { 1.0 }))
                .border_color(color(if on { Token::Ink } else { Token::Rule }))
                .when(on, |v| v.bg(color(Token::Surface)))
                .when(!on && option.enabled, |v| {
                    v.hover(|s| s.border_color(color(Token::Rule2)))
                })
                .into_element()
        }
    }
}

/// One consequence (`.cons dt`, `dd`): the label in spaced capitals over its value (a column of
/// three on desktop, stacked on the phone); a label-value line in a terminal.
fn consequence(app: AppState, label: &'static str, value: Text) -> AnyElement {
    let dt = words(app, label, 11.0, Token::Ink3)
        .uppercase()
        .letter_spacing(px(0.66))
        .number_of_lines(1u32);
    match app.form {
        Form::Terminal => label_value(app, label, value.into_element()),
        Form::Phone => view()
            .mt(px(8.0))
            .gap(px(2.0))
            .child(dt)
            .child(value)
            .into_element(),
        Form::Desktop => view()
            .flex_1()
            .min_w(px(0.0))
            .gap(px(2.0))
            .child(dt)
            .child(value)
            .into_element(),
    }
}

/// The option's number (`.opt .num`): a 20 pt ring, filled ink with the page's colour when
/// chosen. Terminal: `(1)` bold when chosen, as a radio reads without colour.
fn number(app: AppState, label: String, on: bool) -> AnyElement {
    if app.form == Form::Terminal {
        let shown = if on {
            format!("({label})")
        } else {
            format!(" {label} ")
        };
        return text(shown)
            .flex_shrink_0()
            .text_color(color(Token::Ink))
            .when(on, |t| t.font_weight(FontWeight::BOLD))
            .into_element();
    }
    view()
        .size(px(20.0))
        .mt(px(1.0))
        .flex_shrink_0()
        .rounded_full()
        .items_center()
        .justify_center()
        .when(on, |v| v.bg(color(Token::Ink)))
        .when(!on, |v| v.border(px(1.5)).border_color(color(Token::Rule2)))
        .accessibility_elements_hidden(true)
        .child(
            text(label)
                .font_family(FONT_TEXT)
                .text_size(px(11.0))
                .font_weight(FontWeight::BOLD)
                .text_color(color(if on { Token::Bg } else { Token::Ink3 })),
        )
        .into_element()
}

/// "worker recommends" (`.opt .rec`): 11 pt semibold ink-2 in a rule-2 outline.
fn recommends(app: AppState, tag: &'static str) -> AnyElement {
    if app.form == Form::Terminal {
        return chip(app, tag, Hue::Neutral);
    }
    view()
        .px(px(6.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .child(
            words(
                app,
                tag,
                if app.form == Form::Phone { 12.0 } else { 11.0 },
                Token::Ink2,
            )
            .font_weight(FontWeight::SEMIBOLD)
            .number_of_lines(1u32),
        )
        .into_element()
}

/// "see A · diff hunk": underlined quiet text that selects the option's evidence tab.
fn proof_link(app: AppState, option: &AnswerOption) -> Text {
    let link = &option.proof;
    words(
        app,
        link.label.clone(),
        if app.form == Form::Phone { 13.0 } else { 11.5 },
        Token::Ink3,
    )
    .underline()
    .text_decoration_color(color(Token::Rule2))
    .underline_offset(px(2.0))
    .cursor_pointer()
    .role(AccessibilityRole::Link)
    .test_id(&format!("btn-proof-{}", option.number))
    .on_press(press(app, link.action.clone()))
}

/// The alternate answer (`.opt.free`): the pencil in the number's place, a one-line field bound
/// to the state's draft. Locked, it is read-only and faded with the other options.
fn draft_row(app: AppState, part: &Recommendation) -> AnyElement {
    let form = app.form;
    let locked = part.locked;
    let field = input()
        .test_id("opt-draft")
        .label(part.draft_placeholder)
        .placeholder(part.draft_placeholder)
        .readonly(locked)
        .value(move || app.inbox.with(|state| state.draft.clone()))
        .on_change(move |typed: &str| act(app, Action::Draft(typed.to_owned())))
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .bg(AdaptiveColor::TRANSPARENT);
    let field = if form == Form::Terminal {
        field.h(term::rows(1.0))
    } else {
        field
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 15.5 } else { 13.0 }))
            .h(px(if form == Form::Phone { 28.0 } else { 22.0 }))
    };
    let pencil = if form == Form::Terminal {
        text(format!(" {} ", part.draft_mark))
            .flex_shrink_0()
            .text_color(color(Token::Ink3))
            .accessibility_elements_hidden(true)
            .into_element()
    } else {
        view()
            .size(px(20.0))
            .flex_shrink_0()
            .rounded_full()
            .items_center()
            .justify_center()
            .border(px(1.5))
            .border_color(color(Token::Rule2))
            .accessibility_elements_hidden(true)
            .child(code(app, part.draft_mark, 11.0, Token::Ink3))
            .into_element()
    };
    let row = view()
        .flex_row()
        .items_center()
        .opacity(if locked { 0.5 } else { 1.0 })
        .child(pencil)
        .child(field);
    match form {
        Form::Terminal => row.gap(px(8.0)).into_element(),
        Form::Desktop | Form::Phone => row
            .gap(px(10.0))
            .pl(px(10.0))
            .pr(px(14.0))
            .py(px(10.0))
            .rounded(px(if form == Form::Phone { 12.0 } else { 8.0 }))
            .border_1()
            .border_color(color(Token::Rule))
            .into_element(),
    }
}
