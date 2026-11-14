//! The composers of a review (a reply, a new line comment, the request-changes note) and the
//! lines that say where a command sent from here stands.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::review::{Action, Compose, Composer, StatusLine};
use crate::model::{Form, Hue};
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::{dot, hue_ink};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::{Ctx, act, command, rich_at, slug};

/// The least touch target on a phone.
const TARGET: f32 = 44.0;

/// A text field over the page's draft for `composer.draft`, Send, and Cancel when the composer
/// can close. Send keeps the draft unless the reducer actually sent it, so a refusal (offline,
/// no authority, a moved candidate) never loses the owner's words.
pub(super) fn composer(cx: &Ctx, composer: &Composer) -> AnyElement {
    let app = cx.app;
    let form = app.form;
    let drafts = cx.drafts;
    let key = composer.draft.clone();
    let field = {
        let read = key.clone();
        let write = key.clone();
        input()
            .test_id(&format!("field-{}", slug(&key)))
            .label(composer.label)
            .placeholder(composer.placeholder)
            .multiline(true)
            .value(move || drafts.with(|all| all.get(&read).cloned().unwrap_or_default()))
            .on_change(move |text: &str| {
                drafts.update(|all| {
                    all.insert(write.clone(), text.to_owned());
                });
            })
            .flex_1()
            .min_w(px(0.0))
            .text_color(color(Token::Ink))
            .placeholder_color(move || for_appearance(color(Token::Ink4)))
            .caret_color(move || for_appearance(color(Token::Ink)))
            .selection_color(move || for_appearance(color(Token::Sel)))
            .bg(AdaptiveColor::TRANSPARENT)
    };
    let field = match form {
        Form::Terminal => field.h(term::rows(2.0)),
        Form::Desktop => field
            .font_family(FONT_TEXT)
            .text_size(px(12.5))
            .line_height(px(18.0))
            .min_h(px(36.0)),
        Form::Phone => field
            .font_family(FONT_TEXT)
            .text_size(px(15.0))
            .line_height(px(21.0))
            .min_h(px(TARGET)),
    };
    let send = {
        let cx = cx.clone();
        let target = composer.target.clone();
        let key = key.clone();
        move || {
            let text = drafts.with_untracked(|all| all.get(&key).cloned().unwrap_or_default());
            let before = cx.app.review.with_untracked(|state| state.sent.len());
            act(
                &cx,
                Action::Submit {
                    target: target.clone(),
                    text,
                },
            )();
            let sent = cx.app.review.with_untracked(|state| state.sent.len()) > before;
            if sent && target != Compose::Fixture {
                drafts.update(|all| {
                    all.remove(&key);
                });
            }
        }
    };
    let send = button_with(
        app,
        composer.send,
        ButtonKind::Primary,
        ButtonOptions {
            icon: Some(Icon::Send),
            ..ButtonOptions::default()
        },
        format!("btn-send-{}", slug(&key)),
        send,
    );
    let controls = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .children(composer.refusal.as_ref().map(|line| {
            rich_at(
                app,
                &vec![crate::model::span(line.clone())],
                11.5,
                Token::Ink3,
                false,
            )
            .flex_1()
            .text_color(hue_ink(Hue::Held))
            .into_element()
        }))
        .when(composer.refusal.is_none(), |row| {
            row.child(view().flex_1().min_w(px(0.0)))
        })
        .children(composer.cancel.as_ref().map(|cancel| command(cx, cancel)))
        .child(send);
    let shell = view().gap(px(6.0)).child(field).child(controls);
    match form {
        Form::Terminal => term::framed(view().px(term::cells(1.0)).child(shell)).into_element(),
        Form::Desktop | Form::Phone => shell
            .p(px(8.0))
            .rounded(px(if form == Form::Phone { 12.0 } else { 8.0 }))
            .border_1()
            .border_color(color(Token::Rule2))
            .bg(color(Token::Surface))
            .into_element(),
    }
}

/// One sent command: a dot in its tone, what it was and where it stands, then what can still
/// be done (check again, edit again, dismiss). Desktop and phone keep the dot in its own column
/// on the first line, the words wrapping beside it and the actions under them; a terminal sets
/// all of it in one row.
pub(super) fn status_line(cx: &Ctx, line: &StatusLine) -> AnyElement {
    let app = cx.app;
    let words = rich_at(app, &line.text, 12.0, Token::Ink2, false);
    let words = if line.hue == Hue::Neutral {
        words
    } else {
        words.text_color(hue_ink(line.hue))
    };
    let actions = line.actions.iter().map(|button| command(cx, button));
    if app.form == Form::Terminal {
        return view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .child(dot(app, line.hue))
            .child(term::space(1.0))
            .child(words)
            .children(actions)
            .into_element();
    }
    view()
        .flex_row()
        .items_start()
        .gap(px(8.0))
        .child(
            // One line of the 12 pt words high, so the dot sits on the first line.
            view()
                .flex_none()
                .h(px(17.0))
                .justify_center()
                .child(dot(app, line.hue)),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(4.0))
                .child(words)
                .when(!line.actions.is_empty(), |column| {
                    column.child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.0))
                            .children(actions),
                    )
                }),
        )
        .into_element()
}
