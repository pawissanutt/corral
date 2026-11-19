//! The read-only session log of a session without a Pi host (`session::SessionLog`): a run whose
//! harness is not Pi (its pane's screen as corrald captured it) or a headless run that ended (the
//! tail of the `session.log` corrald kept, under an `ended` heading). Its heading and how current
//! it is lead, with Refresh to read the capture again; then the lines, or why there are none
//! (corrald's reason when the log is gone). It takes no input.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{Body, SessionLog, SessionPage};
use crate::model::text::span;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::notice::notice;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::run;
use super::super::transcript::log_lines;
use super::follow_end;

pub(super) fn pager(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let form = app.form;
    let log = computed(move || {
        page.with(|p| match &p.body {
            Body::Log(log) => Some(log.clone()),
            _ => None,
        })
    });
    let head = dynamic(move || log.with(|log| log.as_ref().map(|log| untracked(|| bar(app, log)))));
    let lines =
        dynamic(move || log.with(|log| log.as_ref().map(|log| untracked(|| body(app, log)))));
    let surface = follow_end(
        scroll().test_id("session-log").flex_1().min_h(px(0.0)),
        move || log.with(|_| ()),
    );
    let surface = match form {
        Form::Terminal => surface.child(view().px(term::cells(1.0)).child(lines)),
        Form::Desktop => surface.child(view().px(px(18.0)).py(px(10.0)).child(lines)),
        Form::Phone => surface.child(view().px(px(16.0)).py(px(8.0)).child(lines)),
    };
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(head)
        .child(surface)
        .into_element()
}

/// The pager's head: its heading, how current it is, and Refresh.
fn bar(app: AppState, log: &SessionLog) -> AnyElement {
    let form = app.form;
    let refresh = log.refresh.clone();
    let words = |content: String, ink: Token, mono: bool| {
        let words = text(content)
            .number_of_lines(1u32)
            .truncate()
            .min_w(px(0.0))
            .text_color(color(ink));
        match form {
            Form::Terminal => words,
            _ => words
                .font_family(if mono { FONT_MONO } else { FONT_TEXT })
                .text_size(px(12.0)),
        }
    };
    let row = view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 10.0 }))
        .child(words(log.heading.clone(), Token::Ink2, true).flex_shrink_0())
        .child(words(log.status.clone(), Token::Ink3, false).flex_1())
        .child(button(
            app,
            "Refresh",
            ButtonKind::Quiet,
            "btn-refresh-log",
            move || run(app, refresh.clone()),
        ));
    match form {
        Form::Terminal => view()
            .flex_none()
            .child(row.px(term::cells(1.0)))
            .child(term::hrule())
            .into_element(),
        Form::Desktop => row
            .flex_none()
            .h(px(36.0))
            .px(px(18.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .bg(color(Token::BgSunk))
            .into_element(),
        Form::Phone => row
            .flex_none()
            .px(px(16.0))
            .py(px(6.0))
            .bg(color(Token::BgSunk))
            .into_element(),
    }
}

/// The lines, or what the empty pager says, then corrald's reason when the log is gone.
fn body(app: AppState, log: &SessionLog) -> AnyElement {
    let form = app.form;
    let empty = log.empty.clone().map(|words| {
        let words = text(words).text_color(hue_ink(Hue::Neutral));
        if form == Form::Terminal {
            words.into_element()
        } else {
            words
                .font_family(FONT_TEXT)
                .text_size(px(13.0))
                .into_element()
        }
    });
    let note = log
        .note
        .clone()
        .map(|(hue, words)| notice(app, hue, &vec![span(words)], None));
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .children(note)
        .children(empty)
        .child(log_lines(app, &log.lines))
        .into_element()
}
