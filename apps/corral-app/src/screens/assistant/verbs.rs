//! The canonical record verbs of an assistant session (`work::record_verbs`), such as Revoke
//! permissions: each a quiet outlined button that starts its verb through the work reducer (a
//! verb with fields opens the one verb composer the shells host, a confirm-class verb asks this
//! device first), with the model's reason after a verb that cannot run now. The fixture has no
//! daemon records and shows none.

use nsh_core::prelude::*;

use crate::model::work::{self, Press, Record, VerbAction};
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The verbs of assistant session `id` (tracked: they follow the feed), in a wrapping row; one
/// whose reason says it cannot run stays visible, dimmed, with the reason after it.
pub fn session_verbs(app: AppState, id: Id) -> AnyElement {
    let record = Record::Assistant(id);
    dynamic(move || {
        let verbs = app.feed.with(|feed| work::record_verbs(feed, &record));
        (!verbs.is_empty()).then(|| {
            view()
                .test_id("assistant-verbs")
                .children(
                    verbs
                        .into_iter()
                        .map(|item| verb(app, item))
                        .collect::<Vec<_>>(),
                )
                .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
                .into_element()
        })
    })
}

fn verb(app: AppState, verb: VerbAction) -> AnyElement {
    let VerbAction {
        slug,
        label,
        enabled,
        reason,
        press,
        ..
    } = verb;
    let face = button_with(
        app,
        label,
        ButtonKind::Ghost,
        ButtonOptions {
            disabled: !enabled,
            ..ButtonOptions::default()
        },
        format!("btn-{slug}"),
        move || start(app, press.clone()),
    );
    let why = reason.filter(|_| !enabled).map(|reason| {
        let why = text(reason)
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .text_color(color(Token::Ink3));
        match app.form {
            Form::Terminal => why.into_element(),
            Form::Desktop => why
                .font_family(FONT_TEXT)
                .text_size(px(11.5))
                .into_element(),
            Form::Phone => why
                .font_family(FONT_TEXT)
                .text_size(px(13.0))
                .into_element(),
        }
    });
    let row = view().flex_row().flex_wrap().items_center();
    let row = if app.form == Form::Terminal {
        row.child(face)
            .children(why.map(|why| view().flex_row().child(term::space(2.0)).child(why)))
    } else {
        row.gap(px(8.0)).child(face).children(why)
    };
    row.into_element()
}

/// Starts a verb through the work reducer, which owns the composer, the confirmation and the
/// command's identity.
fn start(app: AppState, press: Press) {
    app.reduce(app.work, move |state, feed, _selection, env| {
        state.press(press, feed, env)
    });
}
