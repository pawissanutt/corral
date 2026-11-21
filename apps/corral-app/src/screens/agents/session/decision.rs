//! What takes the composer's place besides an answer card: a decision card (what an extension
//! asks through a select, confirm or input dialog) and the one confirmation a destructive option
//! asks for on this device.
//!
//! A decision card shows the caller's question in its head with the time left (Held from 30 s),
//! the caller's message (an edit approval's exact text in mono), its numbered options or its one
//! text row with Use, and Cancel (No for a confirm), which answers nothing. For `OPEN_GUARD_MS`
//! after it appears it ignores Enter, digits and presses, so a key meant for what was there
//! before does not answer it. The confirmation names the option, says what it does, and starts
//! with the keys on Cancel. test ids `dock-<card>`, `field-answer-<card>-0`, `btn-card-dismiss`,
//! `btn-card-use`, `card-confirm`, `btn-card-cancel`, `btn-card-confirm`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::session::{
    Action, DecisionCard, DecisionKind, OPEN_GUARD_MS, SessionPage, TextRow,
};
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::run;
use super::cards::{
    TARGET, foot_row, frame, hint, options_list, press, quiet, slot, standing, tint, words,
};

/// The decision card in the composer's place (`dock.decision`), built once per card.
pub(super) fn decision(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let card = computed(move || page.with(|p| p.dock.decision.clone()));
    let id = computed(move || card.with(|c| c.as_ref().map(|c| c.card.clone())));
    dynamic(move || id.get().map(|_| untracked(|| decision_card(app, card))))
}

fn decision_card(app: AppState, card: Memo<Option<DecisionCard>>) -> AnyElement {
    let form = app.form;
    let armed = Rc::new(Cell::new(false));
    {
        let armed = armed.clone();
        set_timeout(Duration::from_millis(OPEN_GUARD_MS), move || {
            armed.set(true)
        });
    }
    let test_id = untracked(|| card.with(|c| c.as_ref().map(|c| c.id.clone()).unwrap_or_default()));
    let countdown = computed(move || card.with(|c| c.as_ref().and_then(|c| c.countdown.clone())));
    let countdown = dynamic(move || {
        countdown.get().map(|(hue, said)| {
            untracked(|| {
                text(said)
                    .number_of_lines(1u32)
                    .flex_shrink_0()
                    .text_color(tint(hue))
                    .when(form != Form::Terminal, |t| {
                        t.font_family(FONT_MONO).tabular_nums().text_size(px(12.0))
                    })
                    .into_element()
            })
        })
    });
    let body = computed(move || {
        card.with(|c| {
            c.as_ref()
                .and_then(|c| c.body.clone().map(|body| (body, c.more, c.kind)))
        })
    });
    let body = dynamic(move || {
        let (body, more, kind) = body.get()?;
        Some(untracked(|| {
            // An edit approval's text is shown exactly, in the mono face.
            let exact = kind == DecisionKind::Confirm && form != Form::Terminal;
            let said = words(form, body, color(Token::Ink2))
                .when(exact, |t| t.font_family(FONT_MONO).text_size(px(12.0)));
            let shown = scroll()
                .max_h(if form == Form::Terminal {
                    term::rows(8.0)
                } else {
                    px(180.0)
                })
                .child(said);
            view()
                .gap(px(2.0))
                .child(shown)
                .children(more.then(|| quiet(form, "… more lines are not shown")))
                .into_element()
        }))
    });
    let options =
        computed(move || card.with(|c| c.as_ref().map(|c| c.options.clone()).unwrap_or_default()));
    let selected = untracked(|| card.with(|c| c.as_ref().and_then(|c| c.selected)));
    let rows = options_list(
        app,
        options,
        armed.clone(),
        None,
        false,
        Some(selected.unwrap_or(0)),
    );
    let input_row = computed(move || card.with(|c| c.as_ref().and_then(|c| c.input.clone())));
    let submit = computed(move || card.with(|c| c.as_ref().and_then(|c| c.submit.clone())));
    let has_input = computed(move || input_row.with(Option::is_some));
    let armed_field = armed.clone();
    let field = dynamic(move || {
        has_input.get().then(|| {
            let armed = armed_field.clone();
            untracked(|| {
                input_row
                    .get()
                    .map(|first| text_row(app, first, input_row, armed, submit, true))
            })
        })?
    });
    let state = computed(move || {
        card.with(|c| {
            c.as_ref()
                .map(|c| (c.read.clone(), c.sending, c.inert.clone()))
        })
    });
    let state = dynamic(move || {
        let (read, sending, inert) = state.get()?;
        untracked(|| standing(app, read, sending, inert.as_deref()))
    });
    let foot = computed(move || {
        card.with(|c| {
            c.as_ref()
                .map(|c| (c.kind, c.dismiss.clone(), c.submit.clone(), c.hint.clone()))
        })
    });
    let armed_use = armed.clone();
    let foot = dynamic(move || {
        let (kind, dismiss, submit, keys) = foot.get()?;
        let armed = armed_use.clone();
        Some(untracked(|| {
            let dismiss = press(
                app,
                if kind == DecisionKind::Confirm {
                    "No"
                } else {
                    "Cancel"
                },
                ButtonKind::Ghost,
                "btn-card-dismiss",
                dismiss,
                ButtonOptions {
                    key_hint: Some("Esc"),
                    ..ButtonOptions::default()
                },
            );
            let submit = submit.map(|action| {
                button_with(
                    app,
                    "Use",
                    ButtonKind::Primary,
                    ButtonOptions {
                        wide: form == Form::Phone,
                        ..ButtonOptions::default()
                    },
                    "btn-card-use",
                    move || {
                        if armed.get() {
                            run(app, action.clone());
                        }
                    },
                )
            });
            foot_row(
                app,
                hint(app, &keys),
                [dismiss].into_iter().chain(submit).collect(),
            )
        }))
    });
    slot(
        app,
        frame(
            app,
            &test_id,
            move || card.with(|c| c.as_ref().map(|c| c.title.clone()).unwrap_or_default()),
            Some(countdown),
            vec![body, rows, field, state, foot],
            true,
        ),
    )
}

/// A card's text row, built once: its label, the field showing the model's text (each change
/// one action), Enter running `submit` once `armed`. `focus` gives the field the keys.
pub(super) fn text_row(
    app: AppState,
    first: TextRow,
    row: Memo<Option<TextRow>>,
    armed: Rc<Cell<bool>>,
    submit: Memo<Option<Action>>,
    focus: bool,
) -> AnyElement {
    let form = app.form;
    let entry = element_ref();
    if focus {
        after_answer(move || entry.focus());
    }
    let displayed = move || row.with(|r| r.as_ref().map(|r| r.text.clone()).unwrap_or_default());
    let field = copyable_input(app, Some(entry), displayed)
        .test_id(&first.id)
        .label(first.label)
        .placeholder(first.placeholder)
        .on_change(move |typed: &str| {
            let action = row.with_untracked(|r| r.as_ref().map(|r| r.with(typed.to_owned())));
            if let Some(action) = action {
                run(app, action);
            }
        })
        .on_submit(move |_: &str| {
            if !armed.get() {
                return;
            }
            if let Some(action) = submit.get_untracked() {
                run(app, action);
            }
        })
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)));
    let field = match form {
        Form::Terminal => field.bg(AdaptiveColor::TRANSPARENT).into_element(),
        Form::Desktop | Form::Phone => field
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 16.0 } else { 13.0 }))
            .min_h(px(if form == Form::Phone { TARGET } else { 30.0 }))
            .px(px(8.0))
            .rounded(px(6.0))
            .bg(color(Token::BgSunk))
            .border_1()
            .border_color(color(Token::Rule2))
            .focus(|s| s.border_color(color(Token::Focus)))
            .into_element(),
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(quiet(form, first.label).flex_shrink_0())
        .child(field)
        .into_element()
}

/// The one confirmation a destructive option asks for on this device, in the composer's place:
/// `Land on main?`, what it does, Cancel (which has the keys) and the option's own label.
pub(super) fn confirm(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let form = app.form;
    let shown = computed(move || page.with(|p| p.dock.confirm.clone()));
    dynamic(move || {
        shown.get().map(|confirm| {
            untracked(|| {
                let cancel = element_ref();
                after_answer(move || cancel.focus());
                let (label, yes) = confirm.confirm;
                let no = confirm.cancel;
                let pressed = vec![
                    button_with(
                        app,
                        "Cancel",
                        ButtonKind::Ghost,
                        ButtonOptions {
                            focus: Some(cancel),
                            key_hint: Some("Esc"),
                            wide: form == Form::Phone,
                            ..ButtonOptions::default()
                        },
                        "btn-card-cancel",
                        move || run(app, no.clone()),
                    ),
                    press(
                        app,
                        label,
                        ButtonKind::Primary,
                        "btn-card-confirm",
                        yes,
                        ButtonOptions {
                            danger: true,
                            ..ButtonOptions::default()
                        },
                    ),
                ];
                let rows = confirm
                    .detail
                    .map(|detail| words(form, detail, color(Token::Ink2)).into_element())
                    .into_iter()
                    .chain([
                        quiet(form, "Asked once on this device before it goes out.").into_element(),
                        foot_row(app, None, pressed),
                    ])
                    .collect();
                let title = confirm.title;
                slot(
                    app,
                    frame(app, "card-confirm", move || title.clone(), None, rows, true),
                )
            })
        })
    })
}
