//! Bubbles: the owner's messages (and in a child conversation the brief from its parent, the
//! owner's steers and the messages between agents) on the raised block with their lead in
//! place of an avatar, and where an owner message came from muted at the right of its first
//! row; an `ask_user` answer under `you answered`; this device's sends the transcript does not
//! hold yet, with where each stands. A terminal leads with the words and draws no fill.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::Echo;
use crate::model::session::transcript::{Answered, Message};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::text::plain;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::line;
use super::parts::{disclosure, pressable, sections, small};
use super::prose::caret;

/// The raised block the owner's words sit on (desktop and phone), lined up with the other
/// messages' bodies (the avatar's column and its gap).
pub(super) fn block(form: Form, content: View) -> AnyElement {
    let sides = if form == Form::Phone { 16.0 } else { 24.0 };
    view()
        .pl(px(sides + 34.0))
        .pr(px(sides))
        .py(px(6.0))
        .child(
            content
                .min_w(px(0.0))
                .px(px(12.0))
                .py(px(8.0))
                .rounded(px(8.0))
                .bg(color(Token::Surface2)),
        )
        .into_element()
}

/// Where a message came from, muted at the right of its first row.
fn tag(app: AppState, words: String) -> AnyElement {
    let words = text(words)
        .number_of_lines(1u32)
        .truncate()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(color(Token::Ink4));
    if app.form == Form::Terminal {
        return words.text_color(color(Token::Ink3)).into_element();
    }
    words
        .font_family(FONT_TEXT)
        .text_size(px(10.5))
        .into_element()
}

/// A bubble's first row: its lead and time, the tag at the right; a tag with facts behind it
/// opens them (`btn-expand-tag-<message id>`).
fn head(app: AppState, message: &Message) -> View {
    let tagged = message.tag.clone().map(|words| {
        let shown = view()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .min_w(px(0.0))
            .flex_shrink(1.0)
            .child(tag(app, words.clone()))
            .when(message.tag_facts.is_some(), |row| {
                row.child(disclosure(
                    app,
                    message.tag_facts.as_ref().is_some_and(|facts| facts.open),
                ))
            });
        match &message.tag_facts {
            Some(facts) => pressable(
                app,
                shown,
                Some(&facts.action),
                format!("Sent from {words}"),
                facts.open,
                &format!("tag-{}", message.id),
            ),
            None => shown,
        }
    });
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .min_w(px(0.0))
        .child(line(app, &message.head, 11.5, Token::Ink3))
        .when(message.live, |head| head.child(caret(app)))
        .child(view().flex_1().min_w(px(0.0)))
        .children(tagged)
}

/// The facts behind a bubble's tag, while opened.
fn tag_facts(app: AppState, message: &Message) -> Option<AnyElement> {
    let facts = message.tag_facts.as_ref().filter(|facts| facts.open)?;
    Some(
        view()
            .mb(px(if app.form == Form::Terminal { 0.0 } else { 4.0 }))
            .child(sections(app, &facts.body))
            .into_element(),
    )
}

/// A bubble message: its head, then its items, then the attachments it carries (each a chip; an
/// image or file opens its readback under them).
pub(super) fn bubble(app: AppState, message: &Message, items: Vec<AnyElement>) -> AnyElement {
    let entry = message.id.clone();
    let carried = computed(move || {
        app.feed.with(|feed| {
            app.assistant
                .with(|state| crate::model::assistant::message_attachments(state, feed, &entry))
        })
    });
    let files = dynamic(move || {
        let (chips, opened) = carried.get();
        (!chips.is_empty()).then(|| {
            untracked(|| {
                view()
                    .gap(px(4.0))
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .gap(px(6.0))
                            .children(super::attachments::chips(app, &chips)),
                    )
                    .children(opened.map(|opened| super::attachments::readback(app, &opened)))
                    .into_element()
            })
        })
    });
    if app.form == Form::Terminal {
        return view()
            .child(head(app, message))
            .child(
                view()
                    .pl(term::cells(2.0))
                    .children(tag_facts(app, message))
                    .children(items)
                    .child(files),
            )
            .into_element();
    }
    block(
        app.form,
        view()
            .child(head(app, message).mb(px(3.0)))
            .children(tag_facts(app, message))
            .children(items)
            .child(files),
    )
}

/// An `ask_user` answer: each question muted on one row (whole once opened), the choice bold,
/// any further text plain. Pressing it opens or folds the questions.
pub(super) fn answered(app: AppState, answered: &Answered, id: &str) -> AnyElement {
    let form = app.form;
    let rows = answered.answers.iter().map(|answer| {
        let question = (!answer.question.is_empty()).then(|| {
            let words = small(app, answer.question.clone(), Token::Ink3);
            if answered.open {
                words
            } else {
                words.truncate()
            }
        });
        let choice = (!answer.choice.is_empty()).then(|| {
            let words = text(answer.choice.clone())
                .text_color(color(Token::Ink))
                .font_weight(if form == Form::Terminal {
                    FontWeight::BOLD
                } else {
                    FontWeight::SEMIBOLD
                });
            if form == Form::Terminal {
                words
            } else {
                words.font_family(FONT_TEXT).text_size(px(13.5))
            }
        });
        view()
            .min_w(px(0.0))
            .mb(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
            .children(question)
            .children(choice)
            .children(answer.more.iter().map(|more| plain(app, more.clone())))
    });
    let block = view().min_w(px(0.0)).children(rows);
    view()
        .min_w(px(0.0))
        .child(pressable(
            app,
            block,
            Some(&answered.action),
            "Your answer".into(),
            answered.open,
            id,
        ))
        .into_element()
}

/// This device's send the transcript does not hold yet: the owner's block with its words, then
/// where it stands (`sending…`, `queued · 2 in line`, `refused: <reason>`) under it, nothing once
/// it started. A child steer names its receiver in the lead. Its command id is this device's
/// own bookkeeping and never shows.
pub(super) fn echo(app: AppState, sent: &Echo) -> AnyElement {
    let form = app.form;
    let lead = match &sent.to {
        Some(child) => format!("you → {child}"),
        None => "you".to_owned(),
    };
    let state = (!sent.words.is_empty()).then(|| {
        let words = text(sent.words.clone()).text_color(if sent.hue == Hue::Neutral {
            color(Token::Ink3)
        } else {
            hue_ink(sent.hue)
        });
        if form == Form::Terminal {
            words.into_element()
        } else {
            words
                .font_family(FONT_TEXT)
                .text_size(px(11.5))
                .into_element()
        }
    });
    let words = plain(app, sent.text.clone());
    if form == Form::Terminal {
        return view()
            .test_id(&sent.id)
            .child(text(lead).flex_shrink_0().text_color(color(Token::Ink3)))
            .child(
                crate::ui::composer::terminal_selectable(app, view().selectable(true))
                    .pl(term::cells(2.0))
                    .child(words.select_text())
                    .children(state),
            )
            .into_element();
    }
    let head = text(lead)
        .mb(px(3.0))
        .font_family(FONT_TEXT)
        .text_size(px(11.5))
        .text_color(color(Token::Ink3));
    view()
        .test_id(&sent.id)
        .child(block(
            form,
            view()
                .child(head)
                .child(words)
                .children(state.map(|state| view().mt(px(4.0)).child(state).into_element())),
        ))
        .into_element()
}
