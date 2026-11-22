//! The Board tab of a build (`session::BoardTab`): where the board's read stands, the plan the
//! build proposes (its reasons and every task with its brief, Review plan docking its choice),
//! the tasks the build filed, each row opening the task's own page, and the tab's controls
//! (Refresh, Plan board tasks from this conversation…, Build record). A control the host cannot
//! run now stays drawn, muted, with its reason. The build record replaces the board in a pager
//! with its way back. Desktop and phone: two-line rows edge to edge, the plan in a bordered
//! card; terminal: one row of Cells per task, the plan under a rule. test ids `session-board`,
//! `task-<id>`, `board-plan`, `plan-task-<key>`, `btn-board-<control>`, `session-record`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{
    BoardControl, BoardRow, BoardTab, Body, PlanCard, PlanTask, RecordLine, RecordPager,
    RecordSection, SessionPage,
};
use crate::model::text::span;
use crate::screens::markdown::{markdown, provide_styles};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::hint::hinted;
use crate::ui::kit::notice::notice;
use crate::ui::kit::rows::{list_row, list_row_tall};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, Token, color};

use super::super::run;
use super::cards::{quiet, tint, words};
use super::tabs::part;
use super::{hue_words, state_mark};

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || {
        page.with(|p| match &p.body {
            Body::Board(view) => Some(view.clone()),
            _ => None,
        })
    });
    let paged = computed(move || {
        shown.with(|board| board.as_ref().is_some_and(|board| board.pager.is_some()))
    });
    dynamic(move || {
        if paged.get() {
            untracked(|| record(app, shown))
        } else {
            untracked(|| board(app, shown))
        }
    })
}

/// The form's page inset around a block that is not a row.
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

fn surface(app: AppState, test_id: &str, content: AnyElement) -> AnyElement {
    let pad = match app.form {
        Form::Terminal => 0.0,
        Form::Desktop => 12.0,
        Form::Phone => 10.0,
    };
    scroll()
        .test_id(test_id)
        .flex_1()
        .min_h(px(0.0))
        .child(view().py(px(pad)).child(content))
        .into_element()
}

fn board(app: AppState, shown: Memo<Option<BoardTab>>) -> AnyElement {
    let content = dynamic(move || {
        shown.with(|board| {
            board
                .as_ref()
                .map(|board| untracked(|| content(app, board)))
        })
    });
    surface(app, "session-board", content)
}

fn content(app: AppState, board: &BoardTab) -> AnyElement {
    let form = app.form;
    let mut out = vec![inset(app, part(app, &board.part, "board"))];
    if let Some(why) = &board.unavailable {
        out.push(inset(
            app,
            notice(
                app,
                Hue::Held,
                &vec![span(format!(
                    "The board is not available in this session: {why}"
                ))],
                None,
            ),
        ));
    }
    if let Some(plan) = &board.plan {
        out.push(inset(app, plan_card(app, plan)));
    }
    if !board.rows.is_empty() {
        let mut head = vec![h3(app, "Tasks", None)];
        if let Some((counts, hue)) = &board.counts {
            head.push(
                words(form, counts.clone(), hue_words(form, *hue))
                    .test_id("board-counts")
                    .into_element(),
            );
        }
        out.push(inset(
            app,
            view().gap(px(2.0)).children(head).into_element(),
        ));
        out.push(
            view()
                .role(AccessibilityRole::List)
                .label("Tasks from this session")
                .children(
                    board
                        .rows
                        .iter()
                        .map(|row| task_row(app, row))
                        .collect::<Vec<_>>(),
                )
                .into_element(),
        );
    }
    if let Some(empty) = board.empty {
        out.push(inset(app, quiet(form, empty).into_element()));
    }
    let controls = [&board.refresh, &board.request];
    let mut pressed: Vec<AnyElement> = controls
        .iter()
        .map(|control| control_button(app, control))
        .collect();
    let record = board.record.clone();
    pressed.push(button_with(
        app,
        "Build record",
        ButtonKind::Ghost,
        ButtonOptions::default(),
        "btn-board-record",
        move || run(app, record.clone()),
    ));
    out.push(inset(
        app,
        view()
            .gap(px(4.0))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .children(pressed),
            )
            .children(controls.iter().filter_map(|control| reason(app, control)))
            .into_element(),
    ));
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 14.0 }))
        .children(out)
        .into_element()
}

/// A control as a ghost button with its terminal key; muted while it cannot run, or while it
/// is on its way.
fn control_button(app: AppState, control: &BoardControl) -> AnyElement {
    let action = control.action.clone();
    let pressed = button_with(
        app,
        if control.busy {
            "Refreshing…"
        } else {
            control.label
        },
        ButtonKind::Ghost,
        ButtonOptions {
            disabled: control.unavailable.is_some() || control.busy,
            key_hint: Some(control.key),
            ..ButtonOptions::default()
        },
        control.id,
        move || run(app, action.clone()),
    );
    match &control.unavailable {
        Some(why) => hinted(
            app,
            pressed,
            format!("{} · unavailable: {why}", control.label),
        ),
        None => pressed,
    }
}

/// Why a control cannot run now, said under the controls (a phone has no hover).
fn reason(app: AppState, control: &BoardControl) -> Option<AnyElement> {
    let why = control.unavailable.as_ref()?;
    Some(
        words(
            app.form,
            format!("{}: unavailable: {why}", control.label),
            tint(Hue::Held),
        )
        .into_element(),
    )
}

fn mono_words(app: AppState, content: String, ink: Token) -> Text {
    let words = text(content).number_of_lines(1u32).text_color(color(ink));
    match app.form {
        Form::Terminal => words,
        Form::Desktop => words.font_family(FONT_MONO).text_size(px(11.5)),
        Form::Phone => words.font_family(FONT_MONO).text_size(px(12.5)),
    }
}

/// One task: id, title and stage; what it does now, its dependencies, the host's detail and its
/// last change under them (desktop and phone), or after them on the one terminal row.
fn task_row(app: AppState, row: &BoardRow) -> AnyElement {
    let form = app.form;
    let open = row.open.clone();
    let selected = row.selected;
    let label = format!("{} {}, {}: {}", row.task, row.title, row.word, row.now);
    let stage = view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(if form == Form::Terminal { 8.0 } else { 5.0 }))
        .children(state_mark(app, row.spinner, row.glyph, row.hue))
        .child(words(form, row.word, hue_words(form, row.hue)).number_of_lines(1u32));
    let id = view()
        .flex_shrink_0()
        .w(match form {
            Form::Terminal => term::cells(8.0),
            Form::Desktop => px(52.0),
            Form::Phone => px(58.0),
        })
        .child(mono_words(app, row.task.to_string(), Token::Ink3).truncate());
    let title = words(form, row.title.clone(), color(Token::Ink))
        .number_of_lines(1u32)
        .truncate()
        .min_w(px(0.0))
        .flex_1();
    let facts: Vec<String> = [
        Some(row.now.clone()),
        Some(row.meta.clone()),
        row.depends.clone(),
        row.detail.clone(),
        row.updated.clone(),
    ]
    .into_iter()
    .flatten()
    .collect();
    let facts = quiet(form, facts.join(" · "))
        .number_of_lines(1u32)
        .truncate()
        .min_w(px(0.0));
    let press = move || run(app, open.clone());
    match form {
        Form::Terminal => list_row(
            app,
            label,
            row.id.clone(),
            move || selected,
            press,
            vec![
                id.into_element(),
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex_row()
                    .gap(px(8.0))
                    .child(title)
                    .into_element(),
                view()
                    .w(term::cells(11.0))
                    .flex_shrink_0()
                    .child(stage)
                    .into_element(),
                view().flex_1().min_w(px(0.0)).child(facts).into_element(),
            ],
        ),
        Form::Desktop | Form::Phone => list_row_tall(
            app,
            label,
            row.id.clone(),
            move || selected,
            press,
            vec![
                id.into_element(),
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(2.0))
                    .child(title.font_weight(FontWeight::MEDIUM))
                    .child(facts)
                    .into_element(),
                stage.into_element(),
            ],
        ),
    }
}

/// The proposed plan: `◆` and its title with Review plan, its facts and reasons, then each task.
fn plan_card(app: AppState, plan: &PlanCard) -> AnyElement {
    let form = app.form;
    let head = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(words(form, "◆", tint(Hue::Held)).flex_shrink_0())
                .child(
                    words(
                        form,
                        format!("A plan is waiting for you: {}", plan.title),
                        color(Token::Ink),
                    )
                    .font_weight(FontWeight::SEMIBOLD)
                    .min_w(px(0.0)),
                ),
        )
        .child(control_button(app, &plan.review));
    provide_styles(app);
    let why = (!plan.why.is_empty()).then(|| markdown(app, &plan.why, false));
    let tasks: Vec<AnyElement> = plan.tasks.iter().map(|task| plan_task(app, task)).collect();
    let block = view()
        .test_id("board-plan")
        .role(AccessibilityRole::Group)
        .label(format!("Proposed plan: {}", plan.title))
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .child(head)
        .child(quiet(form, plan.facts.clone()))
        .children(why)
        .children(reason(app, &plan.review))
        .child(h3(app, "Proposed tasks", None))
        .child(
            view()
                .role(AccessibilityRole::List)
                .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
                .children(tasks),
        );
    match form {
        Form::Terminal => block.into_element(),
        Form::Desktop | Form::Phone => block
            .px(px(14.0))
            .py(px(12.0))
            .rounded(px(10.0))
            .border_1()
            .border_color(color(Token::Rule2))
            .bg(color(Token::Surface))
            .into_element(),
    }
}

/// One proposed task: its key, title and facts, what it comes after, its brief's first line or
/// the whole brief, and how much the host kept back.
fn plan_task(app: AppState, task: &PlanTask) -> AnyElement {
    let form = app.form;
    let head = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_x(px(8.0))
        .child(mono_words(app, task.key.clone(), Token::Ink3).flex_shrink_0())
        .child(
            words(form, task.title.clone(), color(Token::Ink))
                .font_weight(FontWeight::SEMIBOLD)
                .min_w(px(0.0)),
        )
        .children(task.facts.clone().map(|facts| quiet(form, facts)));
    let toggle = task.toggle.clone().map(|action| {
        button_with(
            app,
            if task.open {
                "Hide brief"
            } else {
                "Show brief"
            },
            ButtonKind::Quiet,
            ButtonOptions::default(),
            format!("btn-plan-brief-{}", task.key),
            move || run(app, action.clone()),
        )
    });
    let body = task.body.as_ref().map(|body| markdown(app, body, false));
    // Where the read of the rest stands: a spinner while pages arrive, else why it failed with
    // a Retry; the clipped text stays above either.
    let read = task.read.clone().map(|read| {
        let reading = read.retry.is_none() && read.hue == Hue::Neutral;
        let retry = read.retry.map(|action| {
            button_with(
                app,
                "Retry",
                ButtonKind::Quiet,
                ButtonOptions::default(),
                format!("btn-plan-brief-retry-{}", task.key),
                move || run(app, action.clone()),
            )
        });
        view()
            .test_id(&format!("plan-brief-read-{}", task.key))
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .children(state_mark(app, reading, None, Hue::Corral))
            .child(words(form, read.words, tint(read.hue)).min_w(px(0.0)))
            .children(retry)
    });
    view()
        .test_id(&task.id)
        .gap(px(if form == Form::Terminal { 0.0 } else { 3.0 }))
        .child(head)
        .children(task.depends.clone().map(|depends| quiet(form, depends)))
        .children(task.preview.clone().map(|preview| {
            quiet(form, preview)
                .number_of_lines(1u32)
                .truncate()
                .min_w(px(0.0))
        }))
        .children(body)
        .children(read)
        .children(
            task.more
                .clone()
                .map(|more| words(form, more, tint(Hue::Held))),
        )
        .children(toggle.map(|toggle| view().flex_row().child(toggle)))
        .into_element()
}

/// The build record in place of the board: the way back over the record's sections.
fn record(app: AppState, shown: Memo<Option<BoardTab>>) -> AnyElement {
    let pager = computed(move || shown.with(|board| board.as_ref().and_then(|b| b.pager.clone())));
    let head = dynamic(move || {
        pager.with(|pager| pager.as_ref().map(|pager| untracked(|| bar(app, pager))))
    });
    let body = dynamic(move || {
        pager.with(|pager| {
            pager
                .as_ref()
                .map(|pager| untracked(|| record_body(app, pager)))
        })
    });
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(head)
        .child(surface(app, "session-record", body))
        .into_element()
}

/// The pager's head: back to the board, and what it shows.
fn bar(app: AppState, pager: &RecordPager) -> AnyElement {
    let form = app.form;
    let close = pager.close.clone();
    let row = view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 10.0 }))
        .child(button_with(
            app,
            "‹ Board",
            ButtonKind::Quiet,
            ButtonOptions {
                key_hint: Some("q"),
                ..ButtonOptions::default()
            },
            "btn-record-close",
            move || run(app, close.clone()),
        ))
        .child(
            words(form, "Build record", color(Token::Ink2))
                .number_of_lines(1u32)
                .truncate()
                .min_w(px(0.0)),
        );
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

fn record_body(app: AppState, pager: &RecordPager) -> AnyElement {
    let form = app.form;
    let mut out = vec![part(app, &pager.part, "record")];
    if let Some(record) = &pager.view {
        out.push(
            words(form, record.title.clone(), color(Token::Ink))
                .font_weight(FontWeight::SEMIBOLD)
                .into_element(),
        );
        out.push(record_line(app, &record.outcome, "record-outcome".into()));
        out.extend(
            record
                .sections
                .iter()
                .map(|section| record_section(app, section)),
        );
    }
    inset(
        app,
        view()
            .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
            .children(out)
            .into_element(),
    )
}

fn record_section(app: AppState, section: &RecordSection) -> AnyElement {
    view()
        .test_id(section.id)
        .child(h3(app, section.title, section.note.clone()))
        .child(
            view()
                .role(AccessibilityRole::List)
                .gap(px(if app.form == Form::Terminal { 0.0 } else { 4.0 }))
                .children(
                    section
                        .lines
                        .iter()
                        .enumerate()
                        .map(|(n, line)| record_line(app, line, format!("{}-{n}", section.id)))
                        .collect::<Vec<_>>(),
                ),
        )
        .into_element()
}

/// A record line: its mark in its tone, the words, the detail muted under them; a finding set in
/// under its round.
fn record_line(app: AppState, line: &RecordLine, test_id: String) -> AnyElement {
    let form = app.form;
    let ink = match (line.glyph, line.hue) {
        (_, Hue::Neutral) | (Some(_), _) => color(Token::Ink),
        (None, hue) => hue_words(form, hue),
    };
    let words_line = view()
        .flex_row()
        .items_start()
        .gap(px(if form == Form::Terminal { 8.0 } else { 6.0 }))
        .children(state_mark(app, false, line.glyph, line.hue))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(1.0))
                .child(words(form, line.text.clone(), ink))
                .children(line.detail.clone().map(|detail| quiet(form, detail))),
        );
    let indent = match form {
        Form::Terminal => term::cells(2.0),
        Form::Desktop => px(18.0),
        Form::Phone => px(14.0),
    };
    view()
        .test_id(&test_id)
        .when(line.nested, |line| line.pl(indent))
        .child(words_line)
        .into_element()
}
