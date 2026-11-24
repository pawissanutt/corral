//! The task page's History tab: the task's events newest first, as its detail serves them, and
//! a Comment box that sends the `comment` operation (`field-comment`, `btn-comment-send`).

use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use crate::model::tasks::page;
use crate::model::work;
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::parts::{block, event, quiet};
use crate::screens::cites::rows;

/// The History tab of task `id`.
pub(crate) fn history(app: AppState, id: Id) -> AnyElement {
    let form = app.form;
    let events = {
        let id = id.clone();
        computed(move || app.feed.with(|feed| page::history(feed, id.as_str())))
    };
    let note = {
        let id = id.clone();
        computed(move || app.feed.with(|feed| work::detail_note(feed, id.as_str())))
    };
    let list = dynamic(move || {
        let events = events.get();
        let rows: Vec<AnyElement> = if events.is_empty() {
            let words = note
                .get()
                .map_or_else(|| page::NO_EVENTS.to_owned(), |note| note.text);
            vec![quiet(app, words)]
        } else {
            events
                .iter()
                .map(|line| {
                    view()
                        .child(event(app, line))
                        .children((!line.cites.is_empty()).then(|| rows(app, line.cites.clone())))
                        .into_element()
                })
                .collect()
        };
        block(
            app,
            page::EVENTS,
            None,
            view().test_id("task-history").children(rows).into_element(),
        )
    });
    let body = view().child(comment_box(app, id)).child(list);
    match form {
        Form::Terminal => body.child(term::blank()).into_element(),
        Form::Desktop => body.py(px(16.0)).into_element(),
        Form::Phone => body.px(px(18.0)).py(px(14.0)).into_element(),
    }
}

/// Sends the box's words as a comment, then empties the box; nothing goes out empty.
fn send(app: AppState, draft: Signal<String>, id: &Id) {
    let words = draft.get_untracked();
    if words.trim().is_empty() {
        return;
    }
    draft.set(String::new());
    let id = id.clone();
    app.act(app.work, move |state, _| {
        page::comment(&mut state.ids, &id, &words)
    });
}

/// One raised box with the field and the send; Return in the field sends.
fn comment_box(app: AppState, id: Id) -> AnyElement {
    let form = app.form;
    let draft = signal(String::new());
    let empty = computed(move || draft.with(|words| words.trim().is_empty()));
    let on_enter = id.clone();
    let entry = copyable_input(app, None, move || draft.get())
        .test_id("field-comment")
        .label(format!("Comment on {id}"))
        .placeholder(page::COMMENT)
        .on_change(move |typed: &str| draft.set(typed.to_owned()))
        .on_submit(move |_: &str| send(app, draft, &on_enter))
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT);
    let entry = match form {
        Form::Terminal => entry.h(term::rows(1.0)),
        Form::Desktop => entry.font_family(FONT_TEXT).text_size(px(13.0)).h(px(30.0)),
        Form::Phone => entry
            .font_family(FONT_TEXT)
            .text_size(px(16.0))
            .min_h(px(44.0)),
    };
    let send_button = dynamic(move || {
        let id = id.clone();
        button_with(
            app,
            page::SEND,
            ButtonKind::Ghost,
            ButtonOptions {
                disabled: empty.get(),
                ..ButtonOptions::default()
            },
            "btn-comment-send",
            move || send(app, draft, &id),
        )
    });
    let boxed = view().flex_row().items_center();
    let boxed = match form {
        Form::Terminal => boxed
            .gap(term::cells(1.0))
            .border_1()
            .border_color(color(Token::Rule2))
            .focus_within(|s| s.border_color(color(Token::Focus))),
        Form::Desktop | Form::Phone => boxed
            .max_w(px(760.0))
            .gap(px(8.0))
            .pl(px(12.0))
            .pr(px(6.0))
            .py(px(5.0))
            .rounded(px(8.0))
            .bg(color(Token::Surface))
            .border_1()
            .border_color(color(Token::Rule2))
            .focus_within(|s| s.border_color(color(Token::Focus))),
    };
    view()
        .pb(px(if form == Form::Terminal { 0.0 } else { 16.0 }))
        .child(boxed.child(entry).child(send_button))
        .into_element()
}
