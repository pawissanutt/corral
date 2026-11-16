//! The pinned decide bar under a handover: answer controls while the question is open, the
//! canonical answer's stamp once settled, and its attribution or connection rule.

use nsh_core::keymap::When;
use nsh_core::prelude::*;

use super::{Key, act, button_as, code, live, rich_at, with_source};
use crate::model::Form;
use crate::model::inbox::{self, Action, DecideBar};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::ButtonOptions;
use crate::ui::kit::chip::stamp;
use crate::ui::kit::page::bar;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

fn decide_model(app: AppState, key: Key) -> Memo<DecideBar> {
    computed(move || {
        let env = app.env();
        with_source(app, |state, src, filter| {
            inbox::decide_filtered(state, src, key.as_ref(), env, filter)
        })
    })
}

/// The decide bar, made again as the answer state, the connection or the alternate answer
/// changes. Desktop: the stamp of an answered question first, then the buttons (the answer's
/// shortcut as a key cap beside it), the attribution right-aligned. Phone: the buttons sharing
/// the width. `compact` keeps it to one row of buttons, without the key cap and the
/// attribution ([`decide_note`] shows it at the end of the scroll). Off the phone, Ctrl+Enter
/// runs the answer control while it is enabled, as its key cap says, even from the
/// alternate-answer field.
pub(super) fn decide(app: AppState, key: Key, compact: bool) -> AnyElement {
    let model = decide_model(app, key);
    if app.form != Form::Phone {
        let anywhere = KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        };
        on_key_with("ctrl+enter", anywhere, move |_| {
            if let Some(action) = model.with_untracked(|part| part.submit.clone()) {
                act(app, action);
            }
        });
    }
    live(model, move |part| decide_view(app, part, compact))
}

/// The decide bar's attribution as the last block of a compact sheet's scroll: the command it
/// runs, the connection or authority rule, or the one-answer-wins line.
pub(super) fn decide_note(app: AppState, key: Key) -> AnyElement {
    let model = decide_model(app, key);
    let note = computed(move || model.with(|part| part.note.clone()));
    live(note, move |note| {
        if note.is_empty() {
            return view().into_element();
        }
        let words_of = rich_at(app, &note, 11.5, color(Token::Ink3)).line_height(px(15.5));
        if app.form == Form::Terminal {
            return view()
                .child(term::blank())
                .child(view().px(term::cells(1.0)).child(words_of))
                .into_element();
        }
        view()
            .px(px(24.0))
            .py(px(12.0))
            .child(words_of)
            .into_element()
    })
}

fn decide_view(app: AppState, part: DecideBar, compact: bool) -> AnyElement {
    let form = app.form;
    let mut actions: Vec<AnyElement> = Vec::new();
    if let Some((label, hue)) = part.stamp {
        actions.push(stamp(app, label, hue));
    }
    for model in &part.buttons {
        let asks = matches!(model.action, Action::Thread);
        let options = ButtonOptions {
            // The phone's ask-the-asker button keeps its own width, the others share the rest
            // of the bar. It leads with the agents' glyph: it talks to the worker, not to the
            // assistant that the chat glyph opens elsewhere on the page.
            icon: (form == Form::Phone && asks).then_some(Icon::Agents),
            wide: form == Form::Phone && !asks,
            ..ButtonOptions::default()
        };
        let control = button_as(app, model, options);
        actions.push(match model.shortcut {
            Some(keys) if form == Form::Desktop && !compact => view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(control)
                .child(shortcut(app, keys))
                .into_element(),
            _ => control,
        });
    }
    let note = (!part.note.is_empty() && !compact && form != Form::Phone).then(|| {
        rich_at(app, &part.note, 11.5, color(Token::Ink3))
            .text_right()
            .line_height(px(15.5))
            .into_element()
    });
    if form == Form::Terminal {
        return terminal_bar(actions);
    }
    bar(app, actions, note)
}

/// The terminal's decide bar: a joined line, then the controls whole and two Cells apart,
/// wrapping onto a further row rather than cutting a label where the pane is narrower than the
/// row (`page::bar` keeps one row). test_id "decide-bar", role toolbar.
fn terminal_bar(actions: Vec<AnyElement>) -> AnyElement {
    view()
        .test_id("decide-bar")
        .role(AccessibilityRole::Toolbar)
        .flex_none()
        .child(term::hrule())
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .px(term::cells(1.0))
                .children(actions.into_iter().map(|action| {
                    // 24 pt is two Cells of padding in a terminal.
                    view()
                        .flex_row()
                        .flex_shrink_0()
                        .pr(px(24.0))
                        .child(action)
                        .into_element()
                })),
        )
        .into_element()
}

/// A key cap (`.kbd`) after the answer button: the shortcut that answers.
fn shortcut(app: AppState, keys: &'static str) -> AnyElement {
    view()
        .px(px(4.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .accessibility_elements_hidden(true)
        .child(code(app, keys, 10.5, Token::Ink3).line_height(px(15.0)))
        .into_element()
}
