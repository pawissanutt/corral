//! An owner question's answer card, in the composer's place (`dock.answer`): the card's title
//! with Close, its context, one tab per question and Review (`✓` answered, `○` open), the shown
//! question bold with `· choose any` for a multi-choice one, its numbered options with `●`/`○`,
//! the owner's own words where it takes them, and Send answers, which goes to the first
//! unanswered question until every one has an answer. Review lists each answer or `○
//! unanswered`. Until the card's body is read it is the title and where the read stands, with
//! Retry after a failure. The keys: 1–9 choose, the arrows walk the options, ← → the tabs.
//! test ids `seg-answer-<n>`, `seg-answer-review`, `opt-<card>-<n>`, `field-answer-<card>-<n>`,
//! `review-<n>`, `btn-send-answers`, `btn-answer-close`, `card-read`, `btn-cards-retry`.

use std::cell::Cell;
use std::rc::Rc;

use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{AnswerCard, AnswerTab, SessionPage};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::super::run;
use super::cards::{foot_row, frame, options_list, quiet, slot, standing, words};
use super::decision::text_row;

/// The answer card in the composer's place, built once per card.
pub(super) fn answer(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let card = computed(move || page.with(|p| p.dock.answer.clone()));
    let id = computed(move || card.with(|c| c.as_ref().map(|c| c.card.clone())));
    dynamic(move || id.get().map(|_| untracked(|| answer_card(app, card))))
}

fn answer_card(app: AppState, card: Memo<Option<AnswerCard>>) -> AnyElement {
    let form = app.form;
    let test_id = untracked(|| card.with(|c| c.as_ref().map(|c| c.id.clone()).unwrap_or_default()));
    let close = untracked(|| card.with(|c| c.as_ref().map(|c| c.close.clone())));
    let close = close.map(|action| {
        button_with(
            app,
            "Close",
            ButtonKind::Quiet,
            ButtonOptions {
                key_hint: Some("Esc"),
                ..ButtonOptions::default()
            },
            "btn-answer-close",
            move || run(app, action.clone()),
        )
    });
    let context = computed(move || {
        card.with(|c| c.as_ref().map(|c| (c.meta.clone(), c.body.clone(), c.more)))
    });
    let context = dynamic(move || {
        let (meta, body, more) = context.get()?;
        untracked(|| {
            let rows: Vec<AnyElement> = [
                meta.map(|meta| quiet(form, meta).into_element()),
                body.map(|body| words(form, body, color(Token::Ink2)).into_element()),
                more.then(|| quiet(form, "… more in the transcript").into_element()),
            ]
            .into_iter()
            .flatten()
            .collect();
            (!rows.is_empty()).then(|| view().gap(px(4.0)).children(rows).into_element())
        })
    });
    let tabs =
        computed(move || card.with(|c| c.as_ref().map(|c| c.tabs.clone()).unwrap_or_default()));
    tab_keys(app, tabs);
    let tabs = dynamic(move || {
        let tabs = tabs.get();
        (!tabs.is_empty()).then(|| untracked(|| tab_row(app, &tabs)))
    });
    let shown = computed(move || {
        card.with(|c| {
            c.as_ref()
                .and_then(|c| c.question.as_ref().map(|q| q.index))
        })
    });
    let question = dynamic(move || {
        shown
            .get()
            .map(|index| untracked(|| question(app, card, index)))
    });
    let review = computed(move || card.with(|c| c.as_ref().and_then(|c| c.review.clone())));
    let review = dynamic(move || {
        review.get().map(|review| {
            untracked(|| {
                let rows: Vec<AnyElement> = review
                    .rows
                    .into_iter()
                    .enumerate()
                    .map(|(n, row)| {
                        let (said, ink) = match row.answer {
                            Some(answer) => (answer, color(Token::Ink)),
                            None => ("○ unanswered".to_owned(), hue_ink(Hue::Held)),
                        };
                        let action = row.action;
                        let line = view()
                            .test_id(&format!("review-{n}"))
                            .role(AccessibilityRole::Button)
                            .label(format!("{}: {said}", row.question))
                            .cursor_pointer()
                            .on_press(move || run(app, action.clone()))
                            .gap(px(2.0))
                            .child(quiet(form, row.question))
                            .child(words(form, said, ink).font_weight(FontWeight::SEMIBOLD));
                        match form {
                            Form::Terminal => line.into_element(),
                            _ => line
                                .px(px(8.0))
                                .py(px(5.0))
                                .rounded(px(6.0))
                                .hover(|s| s.bg(color(Token::Hover)))
                                .into_element(),
                        }
                    })
                    .collect();
                view()
                    .role(AccessibilityRole::List)
                    .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
                    .children(rows)
                    .into_element()
            })
        })
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
                .and_then(|c| Some((c.send.clone()?, c.sending || c.inert.is_some())))
        })
    });
    let foot = dynamic(move || {
        let (send, held) = foot.get()?;
        Some(untracked(|| {
            let note = send
                .note
                .map(|note| words(form, note, hue_ink(Hue::Held)).into_element());
            let action = send.action;
            let pressed = button_with(
                app,
                send.label,
                if send.ready {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Ghost
                },
                ButtonOptions {
                    disabled: held,
                    wide: form == Form::Phone,
                    ..ButtonOptions::default()
                },
                send.id,
                move || run(app, action.clone()),
            );
            foot_row(app, note, vec![pressed])
        }))
    });
    slot(
        app,
        frame(
            app,
            &test_id,
            move || card.with(|c| c.as_ref().map(|c| c.title.clone()).unwrap_or_default()),
            close,
            vec![context, tabs, question, review, state, foot],
            true,
        ),
    )
}

/// ← and → show the tab before and after the shown one while no field has the keys.
fn tab_keys(app: AppState, tabs: Memo<Vec<AnswerTab>>) {
    let options = KeyOptions {
        when: When::NotEditing,
        ..KeyOptions::default()
    };
    // Not while a side pane it is drawn in is set aside, hidden over the page.
    app.bind_while_shown(move || {
        for (key, step) in [("left", -1isize), ("right", 1isize)] {
            on_key_with(key, options, move |_| {
                let action = tabs.with_untracked(|tabs| {
                    let at = tabs.iter().position(|tab| tab.selected)?;
                    let next = at.checked_add_signed(step)?;
                    tabs.get(next).map(|tab| tab.action.clone())
                });
                if let Some(action) = action {
                    run(app, action);
                }
            });
        }
    });
}

/// The question tabs: each its label with `✓` or `○`, the shown one marked.
fn tab_row(app: AppState, tabs: &[AnswerTab]) -> AnyElement {
    let form = app.form;
    let items: Vec<AnyElement> = tabs
        .iter()
        .map(|tab| {
            let glyph = if tab.answered { "✓" } else { "○" };
            let ink = if tab.answered {
                hue_ink(Hue::Ok)
            } else {
                color(Token::Ink3)
            };
            let label = text(tab.label.clone())
                .number_of_lines(1u32)
                .text_color(color(if tab.selected {
                    Token::Ink
                } else {
                    Token::Ink2
                }))
                .font_weight(if tab.selected {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                });
            let label = match form {
                Form::Terminal => label,
                _ => label.font_family(FONT_TEXT).text_size(px(12.5)),
            };
            let action = tab.action.clone();
            let item = view()
                .test_id(&tab.id)
                .role(AccessibilityRole::Button)
                .label(format!("{} {glyph}", tab.label))
                .accessibility_state(AccessibilityState {
                    selected: tab.selected,
                    ..AccessibilityState::default()
                })
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .cursor_pointer()
                .on_press(move || run(app, action.clone()))
                .child(text(glyph).text_color(ink).flex_shrink_0())
                .child(label);
            match form {
                Form::Terminal => item
                    .when(tab.selected, |item| item.bg(color(Token::Sel)))
                    .px(term::cells(1.0))
                    .into_element(),
                Form::Desktop | Form::Phone => item
                    .px(px(8.0))
                    .h(px(if form == Form::Phone { 36.0 } else { 26.0 }))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(color(if tab.selected {
                        Token::Ink
                    } else {
                        Token::Rule2
                    }))
                    .hover(|s| s.bg(color(Token::Hover)))
                    .into_element(),
            }
        })
        .collect();
    view()
        .role(AccessibilityRole::Toolbar)
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(if form == Form::Terminal { 8.0 } else { 6.0 }))
        .children(items)
        .into_element()
}

/// The question shown at `index`: its header, the question bold (`· choose any`), its options,
/// and the owner's own words where it takes them, whose Enter sends the card.
fn question(app: AppState, card: Memo<Option<AnswerCard>>, index: usize) -> AnyElement {
    let form = app.form;
    let view_of = move || {
        card.with(|c| {
            c.as_ref()
                .and_then(|c| c.question.clone())
                .filter(|q| q.index == index)
        })
    };
    let asked = untracked(view_of);
    let header = asked
        .as_ref()
        .and_then(|q| q.header.clone())
        .map(|header| quiet(form, header.to_uppercase()).font_weight(FontWeight::SEMIBOLD));
    let asked_text = asked.as_ref().map_or_else(String::new, |q| {
        if q.multi {
            format!("{} · choose any", q.text)
        } else {
            q.text.clone()
        }
    });
    let options = computed(move || view_of().map(|q| q.options).unwrap_or_default());
    let rows = options_list(app, options, Rc::new(Cell::new(true)), None, true, Some(0));
    let row = computed(move || view_of().and_then(|q| q.words));
    let submit = computed(move || {
        card.with(|c| {
            c.as_ref()
                .and_then(|c| c.send.as_ref().map(|send| send.action.clone()))
        })
    });
    // A question that takes only words gives its field the keys.
    let words_only = asked.as_ref().is_some_and(|q| q.options.is_empty());
    let field = asked.and_then(|q| q.words).map(|first| {
        text_row(
            app,
            first,
            row,
            Rc::new(Cell::new(true)),
            submit,
            words_only,
        )
    });
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .children(header)
        .child(
            words(form, asked_text, color(Token::Ink))
                .font_weight(FontWeight::BOLD)
                .test_id(&format!("question-{index}")),
        )
        .child(rows)
        .children(field)
        .into_element()
}
