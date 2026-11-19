//! Ask with no connection to corrald (`DataState::Disconnected`): why nothing can be sent and the
//! way to connect, over a field that still takes words (kept in memory only, never sent or
//! stored) beside a Send that stays dimmed. No thread or composer of the fixture preview shows.
//! A device with an enrolled identity is never here while offline: it boots against its cached
//! store, where the live composer queues what it sends.

use nsh_core::element::input;
use nsh_core::prelude::*;

use crate::model::assistant::UNCONNECTED;
use crate::model::{DataState, Form, Overlay, SettingsSection};
use crate::state::AppState;
use crate::ui::composer::{field_look, focus_on_open};
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::notice::empty;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

/// Tracked: Ask has no connection to send through.
pub(crate) fn unconnected(app: AppState) -> bool {
    app.data.get() == DataState::Disconnected
}

/// What Ask holds under its head without a connection: the explanation with Connect, which
/// closes Ask and opens Settings › Connection, then the field and its dimmed Send.
pub(crate) fn body(app: AppState) -> AnyElement {
    let form = app.form;
    let connect = button(
        app,
        UNCONNECTED.connect,
        ButtonKind::Primary,
        "btn-connect",
        move || {
            app.close(Overlay::Ask);
            app.go_settings(SettingsSection::Connection);
        },
    );
    let explanation = empty(app, UNCONNECTED.title, UNCONNECTED.body, Some(connect));
    let terminal = form == Form::Terminal;
    let foot = view()
        .flex_row()
        .items_end()
        .gap(if terminal { term::cells(1.0) } else { px(6.0) })
        .child(view().flex_1().min_w(px(0.0)).child(field(app)))
        .child(send(app));
    let foot = match form {
        Form::Desktop => foot
            .px(px(14.0))
            .pt(px(10.0))
            .pb(px(12.0))
            .border_t(px(1.0))
            .border_color(color(Token::Rule)),
        Form::Phone => foot.px(px(12.0)).pt(px(2.0)).pb(px(12.0)),
        Form::Terminal => foot.px(term::cells(1.0)).py(term::rows(1.0)),
    };
    // The drawer and the pane give the explanation the height above the field; the phone sheet
    // is as tall as what it holds.
    let fill = form != Form::Phone;
    view()
        .when(fill, |v| v.flex_1().min_h(px(0.0)))
        .child(
            view()
                .when(fill, |v| v.flex_1().min_h(px(0.0)))
                .child(explanation),
        )
        .when(terminal, |v| v.child(term::hrule()))
        .child(foot)
        .into_element()
}

/// The field: Ask's draft, focused as Ask opens like the composer's; nothing it holds is sent.
fn field(app: AppState) -> AnyElement {
    let entry = element_ref();
    focus_on_open(app, entry);
    field_look(
        app.form,
        input()
            .element_ref(&entry)
            .test_id("ask-input")
            .label(UNCONNECTED.placeholder)
            .placeholder(UNCONNECTED.placeholder.to_owned())
            .multiline(true)
            .value(move || app.ask_draft())
            .on_change(move |text: &str| app.set_ask_draft(text.to_owned())),
    )
}

/// Send, dimmed: there is nothing to send through.
fn send(app: AppState) -> AnyElement {
    let phone = app.form == Form::Phone;
    button_with(
        app,
        UNCONNECTED.send,
        ButtonKind::Primary,
        ButtonOptions {
            icon: phone.then_some(Icon::Send),
            icon_only: phone,
            disabled: true,
            ..ButtonOptions::default()
        },
        "btn-send",
        || {},
    )
}
