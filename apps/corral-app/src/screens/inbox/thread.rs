//! The thread with the asker (inline, in the context column and as the phone's sheet) and the
//! history of earlier attempts and decisions.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use super::{act, button, button_as, code, icon_for, live, words};
use crate::model::inbox::{Action, HistoryRow, Notice, ThreadLine, ThreadView};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::ButtonOptions;
use crate::ui::kit::chip::illus;
use crate::ui::kit::marks::dot;
use crate::ui::kit::notice::notice;
use crate::ui::kit::overlay::{Modal, modal};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The thread: its messages (who, time, device; the text), the reply field with Ask, then the
/// follow-up's state. The reply field is bound to the question's follow-up draft and takes
/// focus when "Ask the asker" is pressed.
pub(super) fn thread(app: AppState, part: ThreadView) -> AnyElement {
    let form = app.form;
    let send = &part.send;
    let entry = element_ref();
    let field = input()
        .element_ref(&entry)
        .test_id("thread-reply")
        .label(part.reply_label)
        .placeholder(part.reply_placeholder.clone())
        .readonly(!part.reply_enabled)
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .bg(AdaptiveColor::TRANSPARENT);
    let field = match part.reply_key.clone() {
        Some(key) => {
            focus_on_ask(app, entry);
            let read = key.clone();
            field
                .value(move || {
                    app.feed.with(|feed| {
                        app.inbox
                            .with(|state| crate::model::inbox::reply_value(state, feed, &read))
                    })
                })
                .on_change(move |typed: &str| {
                    act(app, Action::Reply(key.clone(), typed.to_owned()))
                })
        }
        None => field,
    };
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
    let size = if form == Form::Phone { 14.0 } else { 12.5 };
    view()
        .children(
            part.messages
                .iter()
                .map(|message| message_row(app, message)),
        )
        .children(part.empty.map(|none| {
            view()
                .py(px(8.0))
                .child(words(app, none, size, Token::Ink3))
        }))
        .when(form == Form::Terminal, |v| v.child(term::blank()))
        .child(reply)
        .children(part.status.map(|status| status_line(app, status)))
        .children(
            part.attribution
                .map(|tag| view().mt(px(6.0)).child(illus(app, tag))),
        )
        .into_element()
}

/// Focus the reply field each time the owner presses "Ask the asker", after the press's
/// commit so the field is mounted.
fn focus_on_ask(app: AppState, entry: ElementRef) {
    let asked = computed(move || app.inbox.with(|state| state.reply_focus));
    let seen = stored(None::<u32>);
    effect(move || {
        let now = asked.get();
        if seen.replace(Some(now)).is_some_and(|before| before != now) {
            after_answer(move || entry.focus());
        }
    });
}

/// The follow-up's pending, refused or unknown state under the reply, with its recovery.
fn status_line(app: AppState, status: Notice) -> AnyElement {
    view()
        .mt(px(6.0))
        .child(notice(
            app,
            status.hue,
            &status.body,
            status.retry.map(|retry| button(app, &retry)),
        ))
        .into_element()
}

/// One message (`.msg`): who in bold, the time and the device in mono, then the text; a rule
/// under it.
fn message_row(app: AppState, message: &ThreadLine) -> AnyElement {
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
                .child(
                    words(app, message.who.clone(), meta, Token::Ink2)
                        .font_weight(FontWeight::BOLD),
                )
                .child(code(app, message.at.clone(), meta, Token::Ink3))
                .children(
                    message
                        .via
                        .clone()
                        .map(|via| code(app, via, meta, Token::Ink3)),
                ),
        )
        .child(
            words(
                app,
                message.text.clone(),
                if phone { 14.0 } else { 12.5 },
                Token::Ink,
            )
            .line_height(px(if phone { 20.0 } else { 18.0 })),
        )
        .into_element()
}

/// Earlier attempts and decisions: a 2 pt bar (the needs-you colour for the owner's own; in a
/// terminal the needs-you dot or a ring), the date and kind, the text; or what is missing.
pub(super) fn history(
    app: AppState,
    rows: Vec<HistoryRow>,
    none: Option<&'static str>,
) -> AnyElement {
    let form = app.form;
    let size = if form == Form::Phone { 14.0 } else { 12.5 };
    view()
        .children(none.filter(|_| rows.is_empty()).map(|none| {
            view()
                .py(px(6.0))
                .child(words(app, none, size, Token::Ink3))
        }))
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
                        .child(words(app, row.text, size, Token::Ink).line_height(px(size * 1.45))),
                )
                .into_element()
        }))
        .into_element()
}

/// The phone's thread sheet (`.psheet`): the messages and the reply, Done closing it; it follows
/// the thread while open.
pub(super) fn sheet(app: AppState, part: Memo<ThreadView>) -> AnyElement {
    modal(
        app,
        move || app.inbox.with(|state| state.thread_open),
        move || act(app, Action::CloseThread),
        move || {
            let (title, done) =
                part.with_untracked(|part| (part.title.clone(), part.done.label.clone()));
            Modal {
                test_id: "thread-sheet".into(),
                title,
                sub: None,
                body: live(part, move |part| thread(app, part)),
                done: Some(done),
                confirm: None,
            }
        },
    )
}
