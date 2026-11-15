//! A served question's record verbs (escalate, cancel, set a deadline) from the shared table
//! (`work::record_verbs`), under the handover's bands: the ones that can run as buttons, the
//! others dimmed with the model's reason. A verb with fields opens the shared composer the
//! shells host; the fixture preview has no daemon record, so it shows none.

use nsh_core::prelude::*;

use super::{Key, words};
use crate::model::Form;
use crate::model::inbox;
use crate::model::work::{self, Press, Record, VerbAction};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::term;
use crate::ui::tokens::{Token, color};

/// The shown question's verbs, made again as the feed changes what they can do.
pub(super) fn row(app: AppState, key: Key) -> AnyElement {
    let verbs = computed(move || {
        if !app.daemon() {
            return Vec::new();
        }
        app.feed.with(|feed| {
            app.inbox.with(|state| {
                inbox::shown_key(state, feed, key.as_ref())
                    .map(|shown| work::record_verbs(feed, &Record::Question(shown)))
                    .unwrap_or_default()
            })
        })
    });
    dynamic(move || {
        let verbs = verbs.get();
        (!verbs.is_empty()).then(|| untracked(|| drawn(app, verbs)))
    })
}

fn drawn(app: AppState, verbs: Vec<VerbAction>) -> AnyElement {
    let form = app.form;
    let size = if form == Form::Phone { 13.0 } else { 12.0 };
    let reasons: Vec<AnyElement> = verbs
        .iter()
        .filter(|verb| !verb.enabled)
        .filter_map(|verb| Some((verb.label.clone(), verb.reason.clone()?)))
        .map(|(label, reason)| {
            view()
                .flex_row()
                .items_start()
                .gap(if form == Form::Terminal {
                    term::cells(2.0)
                } else {
                    px(8.0)
                })
                .child(
                    words(app, label, size, Token::Ink2)
                        .flex_shrink_0()
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(
                    words(app, reason, size, Token::Ink3)
                        .flex_1()
                        .min_w(px(0.0)),
                )
                .into_element()
        })
        .collect();
    let buttons: Vec<AnyElement> = verbs
        .into_iter()
        .map(|verb| {
            let press = verb.press.clone();
            button_with(
                app,
                verb.label,
                ButtonKind::Ghost,
                ButtonOptions {
                    disabled: !verb.enabled,
                    ..ButtonOptions::default()
                },
                format!("btn-{}", verb.slug),
                move || run(app, press.clone()),
            )
        })
        .collect();
    let row = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(if form == Form::Terminal {
            term::cells(2.0)
        } else {
            px(8.0)
        })
        .children(buttons);
    let list = view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .when(form != Form::Terminal && !reasons.is_empty(), |v| {
            v.pt(px(8.0))
        })
        .children(reasons);
    match form {
        Form::Terminal => view()
            .px(term::cells(1.0))
            .child(term::blank())
            .child(row)
            .child(list)
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .test_id("question-verbs")
            .px(px(if form == Form::Phone { 18.0 } else { 24.0 }))
            .py(px(14.0))
            .border_t_1()
            .border_t_color(color(Token::Rule))
            .child(row)
            .child(list)
            .into_element(),
    }
}

/// Runs a verb through the work reducer, which owns the verbs' composer and commands.
fn run(app: AppState, press: Press) {
    app.reduce(app.work, move |state, feed, _selection, env| {
        state.press(press, feed, env)
    });
}
