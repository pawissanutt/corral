//! The session's background jobs on the Agents tab: their group under the agents (`Jobs · 1
//! running`, then one line per job: its mark, its name (the command's first line) and state with
//! its time), the selected job's detail (the name with the id's first eight characters, its
//! directory, its chips, the last lines of output in the log well,
//! its controls), and an opened job's output in the tab's place: a head that stays put (‹ Agents,
//! the command, how the output stands, how much of it shows, the controls, a stop's
//! confirmation), then the lines in the log pager's face, following the newest bytes while the
//! job runs, with the earlier bytes a press away. test ids `job-<id>`, `session-job-detail`,
//! `session-job-output`, `btn-job-older`, `btn-job-newest`, `btn-retry-job`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{AgentsTab, JobDetail, JobList, JobOutputView, Opened};
use crate::model::text::span;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::mark;
use crate::ui::kit::notice::notice;
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, Token, color};

use super::super::super::run;
use super::super::super::transcript::log_lines;
use super::super::{follow_end, hue_words};
use super::agents_detail::{asked, controls, words};
use super::{cell, pressable};

/// The log well the output lines sit in: sunk on desktop and phone, plain in a terminal.
fn well(app: AppState, content: AnyElement) -> AnyElement {
    match app.form {
        Form::Terminal => content,
        _ => view()
            .px(px(10.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .bg(color(Token::BgSunk))
            .border(px(1.0))
            .border_color(color(Token::Rule))
            .child(content)
            .into_element(),
    }
}

/// The jobs' group.
pub(super) fn list(app: AppState, jobs: &JobList) -> AnyElement {
    let form = app.form;
    let note = jobs.note.clone().map(|(hue, said)| {
        let retry = jobs.retry.clone().map(|action| {
            button(
                app,
                "Retry",
                ButtonKind::Quiet,
                "btn-retry-jobs",
                move || run(app, action.clone()),
            )
        });
        notice(app, hue, &vec![span(said)], retry)
    });
    let rows = jobs.rows.iter().map(|row| {
        let head = view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(mark(app, row.mark, row.hue))
            .child(
                cell(form, row.command.clone(), Token::Ink, true)
                    .flex_1()
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                text(row.state.clone())
                    .number_of_lines(1u32)
                    .flex_shrink_0()
                    .text_color(hue_words(form, row.hue))
                    .when(form != Form::Terminal, |t| {
                        t.font_family(FONT_MONO).text_size(px(12.0))
                    }),
            );
        let label = if row.selected {
            format!(
                "Job {}, {}; press to follow its output",
                row.command, row.state
            )
        } else {
            format!("Job {}, {}", row.command, row.state)
        };
        pressable(
            app,
            &row.test_id,
            label,
            row.selected,
            row.action.clone(),
            view().child(head),
        )
    });
    view()
        .test_id("session-jobs")
        .role(AccessibilityRole::List)
        .label("Background jobs")
        .child(h3(app, jobs.heading.clone(), None))
        .children(note)
        .children(rows.collect::<Vec<_>>())
        .into_element()
}

/// `running · 38s`, `2.0 KB of output`, `⌁ watch /ready/ armed`, each in its tone.
fn chips(app: AppState, chips: &[(Hue, String)]) -> AnyElement {
    let form = app.form;
    view()
        .flex_row()
        .flex_wrap()
        .gap(px(if form == Form::Terminal { 16.0 } else { 6.0 }))
        .children(chips.iter().map(|(hue, said)| {
            let words =
                cell(form, said.clone(), Token::Ink2, true).text_color(hue_words(form, *hue));
            match form {
                Form::Terminal => words.into_element(),
                _ => view()
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded(px(999.0))
                    .border(px(1.0))
                    .border_color(color(Token::Rule2))
                    .child(words)
                    .into_element(),
            }
        }))
        .into_element()
}

/// The selected job's detail.
pub(super) fn detail(app: AppState, detail: &JobDetail) -> AnyElement {
    let form = app.form;
    let tail = (!detail.tail.is_empty()).then(|| well(app, log_lines(app, &detail.tail)));
    view()
        .test_id("session-job-detail")
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .child(
                    cell(form, detail.command.clone(), Token::Ink, true)
                        .flex_1()
                        .font_weight(FontWeight::BOLD),
                )
                .child(cell(form, detail.short.clone(), Token::Ink3, true).flex_shrink_0()),
        )
        .children(
            detail
                .dir
                .clone()
                .map(|dir| cell(form, format!("in {dir}"), Token::Ink3, true)),
        )
        .child(chips(app, &detail.chips))
        .children(tail)
        .child(controls(app, &detail.buttons))
        .into_element()
}

fn opened(tab: &Option<AgentsTab>) -> Option<&JobOutputView> {
    match tab.as_ref()?.opened.as_ref()? {
        Opened::Job(output) => Some(output),
        Opened::Conversation(_) => None,
    }
}

/// The opened job's head and its output.
pub(super) fn output(app: AppState, tab: Memo<Option<AgentsTab>>) -> AnyElement {
    let form = app.form;
    let top = dynamic(move || {
        tab.with(|tab| {
            let asking = tab.as_ref().and_then(|tab| untracked(|| asked(app, tab)));
            opened(tab).map(|output| untracked(|| bar(app, output, asking)))
        })
    });
    let lines =
        dynamic(move || tab.with(|tab| opened(tab).map(|output| untracked(|| body(app, output)))));
    let surface = follow_end(
        scroll()
            .test_id("session-job-output")
            .flex_1()
            .min_h(px(0.0)),
        move || tab.with(|_| ()),
    );
    let surface = match form {
        Form::Terminal => surface.child(view().px(term::cells(1.0)).child(lines)),
        Form::Desktop => surface.child(view().px(px(18.0)).py(px(10.0)).child(lines)),
        Form::Phone => surface.child(view().px(px(12.0)).py(px(8.0)).child(lines)),
    };
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(top)
        .child(surface)
        .into_element()
}

fn bar(app: AppState, output: &JobOutputView, asking: Option<AnyElement>) -> AnyElement {
    let form = app.form;
    let back = output.back.clone();
    let back = button(
        app,
        "‹ Agents",
        ButtonKind::Quiet,
        "btn-agents-back",
        move || run(app, back.clone()),
    );
    let (hue, status) = output.status.clone();
    let who = view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 10.0 }))
        .child(back)
        .child(
            cell(form, output.command.clone(), Token::Ink, true)
                .flex_1()
                .font_weight(FontWeight::BOLD),
        )
        .child(cell(form, output.short.clone(), Token::Ink3, true).flex_shrink_0());
    let mut said = vec![status];
    said.extend(output.span.clone());
    let standing = words(form, said.join(" · "), hue_words(form, hue));
    let column = view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(who)
        .child(standing)
        .children(asking)
        .child(controls(app, &output.buttons));
    match form {
        Form::Terminal => view()
            .flex_none()
            .child(column.px(term::cells(1.0)))
            .child(term::hrule())
            .into_element(),
        _ => column
            .flex_none()
            .px(px(if form == Form::Phone { 12.0 } else { 18.0 }))
            .py(px(if form == Form::Phone { 8.0 } else { 10.0 }))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .bg(color(Token::BgSunk))
            .into_element(),
    }
}

/// The way to earlier bytes, how the read stands, the lines (or what an empty output says), then
/// the way back to the newest bytes.
fn body(app: AppState, output: &JobOutputView) -> AnyElement {
    let form = app.form;
    let older = output.older.clone().map(|action| {
        button(
            app,
            "Earlier output",
            ButtonKind::Quiet,
            "btn-job-older",
            move || run(app, action.clone()),
        )
    });
    let newest = output.newest.clone().map(|action| {
        button(
            app,
            "Newest output",
            ButtonKind::Ghost,
            "btn-job-newest",
            move || run(app, action.clone()),
        )
    });
    let read = output.read.clone().map(|(hue, said)| {
        let retry = output.retry.clone().map(|action| {
            button(
                app,
                "Retry",
                ButtonKind::Quiet,
                "btn-retry-job",
                move || run(app, action.clone()),
            )
        });
        notice(app, hue, &vec![span(said)], retry)
    });
    let empty = output
        .empty
        .clone()
        .map(|said| words(form, said, color(Token::Ink3)).into_element());
    let lines = (!output.lines.is_empty()).then(|| log_lines(app, &output.lines));
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .children(older)
        .children(read)
        .children(empty)
        .children(lines)
        .children(newest)
        .into_element()
}
