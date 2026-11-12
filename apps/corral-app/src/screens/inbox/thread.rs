//! The thread with the asker (inline, in the context column and as the phone's sheet) and the
//! history of earlier attempts and decisions.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use super::{act, button_as, code, icon_for, words};
use crate::model::inbox::{Action, HistoryRow, Message, ThreadView};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::ButtonOptions;
use crate::ui::kit::chip::illus;
use crate::ui::kit::marks::dot;
use crate::ui::kit::overlay::{Modal, modal};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The thread: its messages (who, time, device; the text), the reply field with Ask, the
/// illustrative tag. Sending only posts the model's receipt; the field keeps no state.
pub(super) fn thread(app: AppState, part: ThreadView) -> AnyElement {
    let form = app.form;
    let send = &part.send;
    let field = input()
        .test_id("thread-reply")
        .label(part.reply_label)
        .placeholder(part.reply_placeholder)
        .multiline(true)
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .bg(AdaptiveColor::TRANSPARENT);
    let field = if form == Form::Terminal {
        field.h(term::rows(2.0))
    } else {
        field
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 15.0 } else { 12.5 }))
            .min_h(px(if form == Form::Phone { 48.0 } else { 36.0 }))
    };
    let reply = view()
        .flex_row()
        .items_end()
        .gap(px(8.0))
        .child(field)
        .child(button_as(
            app,
            send,
            ButtonOptions {
                icon: icon_for(&send.action),
                ..ButtonOptions::default()
            },
        ));
    let reply = match form {
        Form::Terminal => reply,
        Form::Desktop | Form::Phone => reply
            .mt(px(10.0))
            .p(px(8.0))
            .rounded(px(if form == Form::Phone { 12.0 } else { 8.0 }))
            .border_1()
            .border_color(color(Token::Rule2))
            .bg(color(Token::Surface)),
    };
    view()
        .children(
            part.messages
                .iter()
                .map(|message| message_row(app, message)),
        )
        .when(form == Form::Terminal, |v| v.child(term::blank()))
        .child(reply)
        .child(view().mt(px(6.0)).child(illus(app, part.attribution)))
        .into_element()
}

/// One message (`.msg`): who in bold, the time and the device in mono, then the text; a rule
/// under it.
fn message_row(app: AppState, message: &Message) -> AnyElement {
    let phone = app.form == Form::Phone;
    let meta = if phone { 12.5 } else { 11.5 };
    view()
        .gap(px(3.0))
        .py(px(8.0))
        .border_b_1()
        .border_b_color(color(Token::Rule))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .child(words(app, message.who, meta, Token::Ink2).font_weight(FontWeight::BOLD))
                .child(code(app, message.at, meta, Token::Ink3))
                .children(message.via.map(|via| code(app, via, meta, Token::Ink3))),
        )
        .child(
            words(
                app,
                message.text,
                if phone { 14.0 } else { 12.5 },
                Token::Ink,
            )
            .line_height(px(if phone { 20.0 } else { 18.0 })),
        )
        .into_element()
}

/// Earlier attempts and decisions: a 2 pt bar (the needs-you colour for the owner's own; in a
/// terminal the needs-you dot or a ring), the date and kind, the text.
pub(super) fn history(app: AppState, rows: Vec<HistoryRow>) -> AnyElement {
    let form = app.form;
    let size = if form == Form::Phone { 14.0 } else { 12.5 };
    view()
        .children(rows.into_iter().map(|row| {
            let lead = if form == Form::Terminal {
                view()
                    .w(term::cells(2.0))
                    .flex_shrink_0()
                    .child(dot(
                        app,
                        if row.owner_decision {
                            Hue::You
                        } else {
                            Hue::Neutral
                        },
                    ))
                    .into_element()
            } else {
                view()
                    .w(px(2.0))
                    .mx(px(3.0))
                    .self_stretch()
                    .flex_shrink_0()
                    .rounded(px(2.0))
                    .bg(color(if row.owner_decision {
                        Token::You
                    } else {
                        Token::Rule2
                    }))
                    .into_element()
            };
            view()
                .flex_row()
                .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
                .py(px(6.0))
                .child(lead)
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap(px(2.0))
                        .child(words(app, row.label, size - 1.5, Token::Ink3))
                        .child(
                            words(app, row.entry.text, size, Token::Ink)
                                .line_height(px(size * 1.45)),
                        ),
                )
                .into_element()
        }))
        .into_element()
}

/// The phone's thread sheet (`.psheet`): the messages and the reply, Done closing it.
pub(super) fn sheet(app: AppState, part: ThreadView) -> AnyElement {
    modal(
        app,
        move || app.inbox.with(|state| state.thread_open),
        move || act(app, Action::CloseThread),
        move || Modal {
            test_id: "thread-sheet".into(),
            title: part.title.into(),
            sub: None,
            body: thread(app, part.clone()),
            done: Some(part.done.label.clone()),
            confirm: None,
        },
    )
}
