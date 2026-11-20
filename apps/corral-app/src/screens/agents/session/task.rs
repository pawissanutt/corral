//! The Task tab of a task-bound session (`session::TaskTab`): where the read of the task's record
//! stands with the ways out (its page, Timeline), then the task's Overview exactly as its page
//! draws it (description, children, dependencies, pipeline, checks, runs, artifacts), then every
//! attempt the engine recorded, this session's marked. test ids `session-task`,
//! `btn-task-<link>`, `attempt-<run>`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::session::{Attempt, Body, SessionPage, TaskLink, TaskTab};
use crate::screens::task::overview::overview;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::super::run;
use super::cards::{quiet, words};
use super::tabs::part;
use super::{hue_words, state_mark};

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || {
        page.with(|p| match &p.body {
            Body::Task(view) => Some(view.clone()),
            _ => None,
        })
    });
    let task = computed(move || shown.with(|tab| tab.as_ref().and_then(|tab| tab.task.clone())));
    let top =
        dynamic(move || shown.with(|tab| tab.as_ref().map(|tab| untracked(|| head(app, tab)))));
    let tried = dynamic(move || {
        shown.with(|tab| {
            tab.as_ref()
                .and_then(|tab| untracked(|| attempts(app, tab)))
        })
    });
    let story_view = dynamic(move || task.get().map(|id| untracked(|| overview(app, id))));
    let pad = match app.form {
        Form::Terminal => 0.0,
        Form::Desktop => 12.0,
        Form::Phone => 10.0,
    };
    scroll()
        .test_id("session-task")
        .flex_1()
        .min_h(px(0.0))
        .child(view().py(px(pad)).child(top).child(story_view).child(tried))
        .into_element()
}

/// The form's page inset around a block the story does not pad itself.
fn inset(app: AppState, child: AnyElement) -> AnyElement {
    let frame = view();
    match app.form {
        Form::Terminal => frame.px(term::cells(1.0)),
        Form::Desktop => frame.px(px(18.0)),
        Form::Phone => frame.px(px(16.0)),
    }
    .child(child)
    .into_element()
}

/// The read's state and the ways out.
fn head(app: AppState, tab: &TaskTab) -> AnyElement {
    let links: Vec<AnyElement> = tab
        .links
        .iter()
        .map(|link| link_button(app, link))
        .collect();
    let links = (!links.is_empty()).then(|| {
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .children(links)
    });
    inset(
        app,
        view()
            .gap(px(if app.form == Form::Terminal { 0.0 } else { 8.0 }))
            .child(part(app, &tab.part, "task"))
            .children(links)
            .into_element(),
    )
}

pub(super) fn link_button(app: AppState, link: &TaskLink) -> AnyElement {
    let action = link.action.clone();
    button_with(
        app,
        link.label,
        ButtonKind::Ghost,
        ButtonOptions {
            key_hint: link.key,
            ..ButtonOptions::default()
        },
        link.id,
        move || run(app, action.clone()),
    )
}

/// Every run the engine recorded for the task, oldest first.
fn attempts(app: AppState, tab: &TaskTab) -> Option<AnyElement> {
    if tab.attempts.is_empty() {
        return None;
    }
    let rows: Vec<AnyElement> = tab
        .attempts
        .iter()
        .map(|attempt| attempt_row(app, attempt))
        .collect();
    Some(inset(
        app,
        view()
            .child(h3(app, "Attempts", Some(tab.attempts.len().to_string())))
            .child(
                view()
                    .role(AccessibilityRole::List)
                    .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
                    .children(rows),
            )
            .into_element(),
    ))
}

fn attempt_row(app: AppState, attempt: &Attempt) -> AnyElement {
    let form = app.form;
    let mut said = attempt.outcome.clone();
    if attempt.this {
        said.push_str(" · this session");
    }
    view()
        .test_id(&attempt.id)
        .flex_row()
        .items_start()
        .gap(px(if form == Form::Terminal { 8.0 } else { 6.0 }))
        .children(state_mark(app, attempt.spinner, attempt.glyph, attempt.hue))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(1.0))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_x(px(8.0))
                        .child(
                            words(form, attempt.text.clone(), color(Token::Ink))
                                .when(attempt.this, |t| t.font_weight(FontWeight::SEMIBOLD)),
                        )
                        .child(words(form, said, hue_words(form, attempt.hue))),
                )
                .child(quiet(form, attempt.when.clone())),
        )
        .into_element()
}
