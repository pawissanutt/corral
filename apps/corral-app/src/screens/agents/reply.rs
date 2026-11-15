//! The words region under a session's surface: the reply box (`.composer`, the phone's
//! `.reply`) or, for an assistant, the shared composer bound to its thread. Send waits for a
//! working session's turn; Steer, offered beside it while the session works, interrupts it.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{Action, Reply, ReplyMode, Session};
use crate::model::assistant::ThreadRef;
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::ask;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::page::bar;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

use super::run;
use super::transcript::line;

/// The words region for the page's form: an assistant's composer on its thread, else the reply
/// box (docked in the phone's bar). Each is redrawn only when what it shows changes, not with
/// each new screen or transcript page, so a field being typed in keeps its focus.
pub fn region(app: AppState, session: Memo<Session>, draft: Signal<String>) -> AnyElement {
    let chat = computed(move || session.with(|s| s.chat.clone()));
    let shown = computed(move || session.with(|s| s.reply.clone()));
    dynamic(move || match chat.get() {
        Some(scope) => untracked(|| ask::docked(app, ThreadRef::Live(scope))),
        None if app.form == Form::Phone => dynamic(move || {
            shown.with(|shown| bar(app, vec![phone_reply(app, shown, draft)], None))
        }),
        None => dynamic(move || shown.with(|shown| reply(app, shown, draft))),
    })
}

/// The reply field: one draft per open session page, kept while the view switches.
fn field(app: AppState, reply: &Reply, draft: Signal<String>) -> AnyElement {
    let form = app.form;
    let size = match form {
        Form::Phone => 15.0,
        Form::Desktop | Form::Terminal => text_size(form),
    };
    let entry = input()
        .test_id("session-reply")
        .label(reply.accessibility_label)
        .multiline(true)
        .placeholder(reply.placeholder.clone())
        .value(move || draft.get())
        .on_change(move |text: &str| draft.set(text.to_owned()))
        .flex_1()
        .min_w(px(0.0))
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT);
    match form {
        Form::Desktop => entry.h(px(38.0)).line_height(px(19.0)).into_element(),
        Form::Phone => entry
            .h(px(40.0))
            .py(px(9.0))
            .line_height(px(21.0))
            .into_element(),
        Form::Terminal => entry.h(term::rows(2.0)).into_element(),
    }
}

/// Send, and Steer before it while the session works: the field's words go to the model's
/// reducer; while a reply is in flight Send waits, dimmed, so one intent never becomes two
/// commands. Steer is the quieter choice: Send stays the main action.
fn sends(app: AppState, reply: &Reply, draft: Signal<String>) -> Vec<AnyElement> {
    let busy = reply.mode == ReplyMode::Busy;
    let steer = reply.steer.map(|label| {
        button(
            app,
            label,
            ButtonKind::Ghost,
            "btn-session-steer",
            move || {
                run(app, Action::Steer(draft.get_untracked()));
            },
        )
    });
    let send = button_with(
        app,
        if busy { "Sending…" } else { reply.button },
        ButtonKind::Primary,
        ButtonOptions {
            icon: Some(Icon::Send),
            disabled: busy,
            ..ButtonOptions::default()
        },
        "btn-send",
        move || run(app, Action::Send(draft.get_untracked())),
    );
    steer.into_iter().chain([send]).collect()
}

/// The latest reply's outcome in its tone, with "Check delivery" when it is unknown.
fn outcome(app: AppState, reply: &Reply) -> Option<AnyElement> {
    let (hue, words) = reply.status.clone()?;
    let ink = match hue {
        Hue::Neutral => color(Token::Ink3),
        hue => hue_ink(hue),
    };
    let words = text(words)
        .test_id("session-reply-status")
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(ink);
    let words = if app.form == Form::Terminal {
        words
    } else {
        words.font_family(FONT_TEXT).text_size(px(11.5))
    };
    Some(
        view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(words)
            .children(reply.recover.clone().map(|action| {
                button(
                    app,
                    "Check delivery",
                    ButtonKind::Quiet,
                    "btn-recover-reply",
                    move || run(app, action.clone()),
                )
            }))
            .into_element(),
    )
}

/// Why replies do not go to this session, with where to answer instead.
fn closed(app: AppState, reason: &str, action: &Option<(&'static str, Action)>) -> AnyElement {
    let go = action.clone().map(|(label, action)| {
        button(
            app,
            label,
            ButtonKind::Ghost,
            "btn-reply-elsewhere",
            move || run(app, action.clone()),
        )
    });
    if app.form == Form::Terminal {
        return view()
            .flex_row()
            .items_center()
            .px(term::cells(1.0))
            .child(term::quiet(reason.to_owned()).flex_1().min_w(px(0.0)))
            .children(go.map(|go| view().flex_row().child(term::space(2.0)).child(go)))
            .into_element();
    }
    view()
        .test_id("session-reply-closed")
        .flex_1()
        .min_w(px(0.0))
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(12.0))
        .py(px(if app.form == Form::Phone { 8.0 } else { 9.0 }))
        .rounded(px(if app.form == Form::Phone { 12.0 } else { 8.0 }))
        .bg(color(Token::BgSunk))
        .border_1()
        .border_color(color(Token::Rule))
        .child(
            text(reason.to_owned())
                .flex_1()
                .min_w(px(0.0))
                .font_family(FONT_TEXT)
                .text_size(px(if app.form == Form::Phone { 14.0 } else { 12.5 }))
                .text_color(color(Token::Ink2)),
        )
        .children(go)
        .into_element()
}

/// The reply box under the transcript: the field, Steer and Send in one raised box, then how
/// the message is sent, the latest reply's outcome and where the open question is answered.
/// Desktop: `.composer` (a rule above, 10 24 12, the box at most 772 pt wide). Terminal: a
/// rule, the field and the sends, the hint quiet. A session that takes no replies says why
/// instead. The phone docks [`phone_reply`] instead.
pub fn reply(app: AppState, reply: &Reply, draft: Signal<String>) -> AnyElement {
    let form = app.form;
    let status = outcome(app, reply);
    if form == Form::Terminal {
        let entry = match &reply.mode {
            ReplyMode::Closed { reason, action } => closed(app, reason, action),
            ReplyMode::Open | ReplyMode::Busy => view()
                .flex_row()
                .items_end()
                .px(term::cells(1.0))
                .child(field(app, reply, draft))
                .children(
                    sends(app, reply, draft)
                        .into_iter()
                        .flat_map(|send| [term::space(2.0), send]),
                )
                .into_element(),
        };
        let takes_words = !matches!(reply.mode, ReplyMode::Closed { .. });
        return view()
            .flex_none()
            .child(term::hrule())
            .child(entry)
            .children(status.map(|status| view().px(term::cells(1.0)).child(status)))
            .when(takes_words, |box_| {
                box_.child(view().px(term::cells(1.0)).child(line(
                    app,
                    &reply.hint,
                    0.0,
                    Token::Ink3,
                )))
            })
            .into_element();
    }
    let entry = match &reply.mode {
        ReplyMode::Closed { reason, action } => view()
            .w_full()
            .max_w(px(772.0))
            .flex_row()
            .child(closed(app, reason, action))
            .into_element(),
        ReplyMode::Open | ReplyMode::Busy => view()
            .w_full()
            .max_w(px(772.0))
            .flex_row()
            .items_end()
            .gap(px(8.0))
            .pl(px(12.0))
            .pr(px(8.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .bg(color(Token::Surface))
            .border_1()
            .border_color(color(Token::Rule2))
            .child(field(app, reply, draft))
            .children(sends(app, reply, draft))
            .into_element(),
    };
    let takes_words = !matches!(reply.mode, ReplyMode::Closed { .. });
    view()
        .flex_none()
        .px(px(24.0))
        .pt(px(10.0))
        .pb(px(12.0))
        .bg(color(Token::Bg))
        .border_t_1()
        .border_t_color(color(Token::Rule))
        .items_center()
        .child(entry)
        .children(status.map(|status| view().w_full().max_w(px(772.0)).mt(px(6.0)).child(status)))
        .when(takes_words, |box_| {
            box_.child(
                view()
                    .w_full()
                    .max_w(px(772.0))
                    .mt(px(6.0))
                    .flex_row()
                    .flex_wrap()
                    .gap(px(12.0))
                    .child(line(app, &reply.hint, 11.0, Token::Ink4))
                    .when(!reply.question_hint.is_empty(), |hints| {
                        hints.child(
                            text(reply.question_hint.clone())
                                .font_family(FONT_TEXT)
                                .text_size(px(11.0))
                                .text_color(color(Token::Ink4)),
                        )
                    }),
            )
        })
        .into_element()
}

/// The phone's docked reply (`.reply`): the field, Steer and Send in one rounded box, for the
/// page's bar, with the latest reply's outcome above it; or why the session takes none.
pub fn phone_reply(app: AppState, reply: &Reply, draft: Signal<String>) -> AnyElement {
    let entry = match &reply.mode {
        ReplyMode::Closed { reason, action } => closed(app, reason, action),
        ReplyMode::Open | ReplyMode::Busy => view()
            .flex_row()
            .items_end()
            .gap(px(8.0))
            .pl(px(12.0))
            .pr(px(6.0))
            .py(px(6.0))
            .rounded(px(12.0))
            .bg(color(Token::Surface))
            .border_1()
            .border_color(color(Token::Rule2))
            .child(field(app, reply, draft))
            .children(sends(app, reply, draft))
            .into_element(),
    };
    view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(6.0))
        .children(outcome(app, reply).map(|status| view().px(px(4.0)).child(status)))
        .child(entry)
        .into_element()
}
