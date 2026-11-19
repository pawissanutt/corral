//! The typed cards an assistant reply carries: the ask_user choice and the host starting.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::assistant::{Action, AskUserCard, Chip, WordsAnswer};
use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::{dot, hue_fill, mark};
use crate::ui::kit::text::{rich, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

use super::parts::{act, frame, meta_size, note_size, ring, strong_run, words};

/// The ask_user card (`.askuser`): who asks and when, the question, the options with their
/// consequences (the recorded one outlined in ink; an open one answers it when pressed), the
/// field for the owner's own words when the question takes them, and how it was answered.
pub fn ask_user(app: AppState, card: &AskUserCard) -> AnyElement {
    let form = app.form;
    // Four or more side by side leave a phone's options a word wide: they stack there.
    let stacked = form == Form::Phone && card.options.len() > 3;
    let options = card.options.iter().enumerate().map(|(at, option)| {
        let selected = option.selected;
        let enabled = option.enabled;
        let label = text(option.label.clone())
            .font_weight(FontWeight::BOLD)
            .text_color(color(if selected { Token::Ink } else { Token::Ink2 }));
        let label = if form == Form::Terminal {
            label
        } else {
            label
                .font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 15.0 } else { 13.0 }))
        };
        let consequence = words(
            app,
            option.consequence.clone(),
            note_size(form),
            Token::Ink3,
        );
        let choice = view()
            .role(AccessibilityRole::Button)
            .label(format!("{}. {}", option.label, option.consequence))
            .accessibility_state(AccessibilityState {
                selected,
                disabled: !enabled,
                ..AccessibilityState::default()
            })
            .test_id(&format!("opt-{}", at + 1))
            .gap(px(3.0))
            .when_some(option.action.clone(), |choice, action| {
                choice
                    .cursor_pointer()
                    .focus_visible(ring)
                    .on_press(move || act(app, action.clone()))
            });
        if form == Form::Terminal {
            choice
                .child(
                    view()
                        .flex_row()
                        .child(
                            text(if selected { "● " } else { "○ " }).text_color(color(Token::Ink)),
                        )
                        .child(label),
                )
                .child(view().pl(term::cells(2.0)).child(consequence))
                .into_element()
        } else {
            choice
                .when(!stacked, |c| c.flex_1())
                .min_w(px(0.0))
                .px(px(10.0))
                .py(px(8.0))
                .rounded(px(8.0))
                .border(px(if selected { 1.5 } else { 1.0 }))
                .border_color(color(if selected { Token::Ink } else { Token::Rule2 }))
                .when(enabled, |c| c.hover(|s| s.bg(color(Token::Hover))))
                .when(!enabled && !selected, |c| c.opacity(0.8))
                .child(label)
                .child(consequence)
                .into_element()
        }
    });
    let options = if form == Form::Terminal {
        view().children(options.collect::<Vec<_>>())
    } else if stacked {
        view().gap(px(8.0)).children(options.collect::<Vec<_>>())
    } else {
        view()
            .flex_row()
            .items_stretch()
            .gap(px(8.0))
            .children(options.collect::<Vec<_>>())
    };
    let mut parts = vec![
        asks_head(app, &card.heading, None),
        rich(app, &card.question),
    ];
    // A question that takes only the owner's own words has no options row.
    if !card.options.is_empty() {
        parts.push(options.into_element());
    }
    if let Some(words) = &card.words {
        parts.push(words_answer(app, words));
    }
    if let Some(answer) = &card.answer {
        parts.push(rich_small(app, answer));
    }
    frame(app, parts)
}

/// The owner's own words as the answer: a field and Answer, Enter sending too. Blank words
/// send nothing. test_id `field-answer` and `btn-answer`.
fn words_answer(app: AppState, words: &WordsAnswer) -> AnyElement {
    let form = app.form;
    let typed = signal(String::new());
    let send = {
        let session = words.session.clone();
        let question = words.question.clone();
        move || {
            act(
                app,
                Action::AnswerQuestion {
                    session: session.clone(),
                    question: question.clone(),
                    option: None,
                    text: Some(typed.get_untracked()),
                },
            )
        }
    };
    let entry = input()
        .test_id("field-answer")
        .label(words.placeholder.clone())
        .placeholder(words.placeholder.clone())
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .text_color(color(Token::Ink))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .flex_1()
        .min_w(px(0.0))
        .on_change(move |text: &str| typed.set(text.to_owned()))
        .on_submit({
            let send = send.clone();
            move |_: &str| send()
        });
    let entry = match form {
        Form::Terminal => entry.bg(AdaptiveColor::TRANSPARENT).into_element(),
        Form::Desktop | Form::Phone => entry
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 15.0 } else { 13.0 }))
            .bg(color(Token::BgSunk))
            .border_1()
            .border_color(color(Token::Rule2))
            .rounded(px(6.0))
            .px(px(8.0))
            .py(px(5.0))
            .into_element(),
    };
    let answer = button_with(
        app,
        "Answer",
        ButtonKind::Primary,
        ButtonOptions::default(),
        "btn-answer",
        send,
    );
    row(app, vec![entry, answer], 8.0).into_element()
}

/// The head of a card that asks (`.askuser .hd`): the needs-you dot, the heading, a chip at the
/// trailing edge.
fn asks_head(app: AppState, heading: &str, state: Option<&Chip>) -> AnyElement {
    let mut parts = vec![
        dot(app, Hue::You),
        words(app, heading.to_owned(), meta_size(app.form), Token::Ink3)
            .flex_1()
            .min_w(px(0.0))
            .into_element(),
    ];
    if let Some(state) = state {
        parts.push(chip(app, state.label.clone(), state.hue));
    }
    row(app, parts, 6.0).into_element()
}

/// The host starting (`.hoststate`): the running mark, the state in bold and what it means, on
/// the corral ground.
pub fn starting(app: AppState, state: &Chip, explanation: &str) -> AnyElement {
    let form = app.form;
    let size = if form == Form::Phone {
        15.0
    } else {
        text_size(form)
    };
    let sentence = words(app, "", size, Token::Ink)
        .flex_1()
        .min_w(px(0.0))
        .child(strong_run(state.label.clone()))
        .child(text(format!(" · {explanation}")));
    let line = view()
        .flex_row()
        .items_center()
        .bg(hue_fill(state.hue))
        .child(mark(app, Mark::Active, state.hue))
        .when(form == Form::Terminal, |r| r.child(term::space(1.0)))
        .child(sentence);
    if form == Form::Terminal {
        return line.into_element();
    }
    line.gap(px(10.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(8.0))
        .into_element()
}

/// Pieces in a wrapping row, `gap` points apart (one Cell in a terminal).
fn row(app: AppState, parts: Vec<AnyElement>, gap: f32) -> View {
    let base = view().flex_row().flex_wrap().items_center();
    if app.form != Form::Terminal {
        return base.gap_x(px(gap)).gap_y(px(4.0)).children(parts);
    }
    let mut spaced = Vec::with_capacity(parts.len() * 2);
    for (at, part) in parts.into_iter().enumerate() {
        if at > 0 {
            spaced.push(term::space(1.0));
        }
        spaced.push(part);
    }
    base.children(spaced)
}
