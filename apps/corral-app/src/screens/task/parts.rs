//! What the task page and the task inspector are built from: a block under its caption (`.blk`,
//! `.dsec`), a linked row that opens what it names (`.kv` as a link: a child, a dependency, a
//! run's session), a plain labelled row and a quiet line.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::tasks::page::{EventLine, Linked, RunLine};
use crate::state::AppState;
use crate::ui::kit::status::status_mark;
use crate::ui::kit::text::{dim, h3_flush};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// A desktop row of a block, and the phone's least touch target.
const ROW: f32 = 28.0;
const TARGET: f32 = 44.0;

/// A block: its caption, then its content; 16 pt under it on desktop and phone, a blank row in
/// a terminal.
pub(crate) fn block(
    app: AppState,
    caption: impl Into<SharedString>,
    note: Option<String>,
    content: AnyElement,
) -> AnyElement {
    let caption: SharedString = caption.into();
    match app.form {
        Form::Terminal => view()
            .child(
                view()
                    .flex_row()
                    .gap(term::cells(1.0))
                    .child(
                        text(caption)
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(Token::Ink)),
                    )
                    .children(note.map(term::quiet)),
            )
            .child(content)
            .child(term::blank())
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .pb(px(16.0))
            .child(h3_flush(app, caption, note))
            .child(content)
            .into_element(),
    }
}

/// A quiet line where a block has nothing to list.
pub(crate) fn quiet(app: AppState, words: impl Into<SharedString>) -> AnyElement {
    dim(app, words).into_element()
}

/// One row: a leading part (a mark and an id), the main words cut to fit, and a quiet
/// trailing part.
fn cells(app: AppState, lead: Vec<AnyElement>, main: String, trail: String) -> Vec<AnyElement> {
    let form = app.form;
    let main = text(main)
        .truncate()
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink2));
    let trail = text(trail)
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(Token::Ink3));
    let (main, trail) = match form {
        Form::Terminal => (main, trail),
        Form::Desktop | Form::Phone => (
            main.font_family(FONT_TEXT).text_size(px(13.0)),
            trail.font_family(FONT_TEXT).text_size(px(12.0)),
        ),
    };
    let mut out = lead;
    out.push(main.into_element());
    out.push(trail.into_element());
    out
}

/// A row that names something and opens it on a press, as a link does (`test_id`).
pub(crate) fn link_row(
    app: AppState,
    test_id: String,
    spoken: String,
    lead: Vec<AnyElement>,
    main: String,
    trail: String,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let handle = element_ref();
    let row = view()
        .element_ref(&handle)
        .test_id(&test_id)
        .role(AccessibilityRole::Link)
        .label(spoken)
        .flex_row()
        .items_center()
        .cursor_pointer()
        .on_press(on_press);
    let content = cells(app, lead, main, trail);
    match form {
        Form::Terminal => row
            .h(term::rows(1.0))
            .gap(term::cells(1.0))
            .child(term::focus_cell(term::focus_of(handle)))
            .children(content)
            .into_element(),
        Form::Desktop | Form::Phone => row
            .min_h(px(if form == Form::Phone { TARGET } else { ROW }))
            .gap(px(8.0))
            .px(px(4.0))
            .rounded(px(4.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(2.0))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-2.0))
            })
            .children(content)
            .into_element(),
    }
}

/// A row that only says: a label, words and a quiet trailing part.
pub(crate) fn plain_row(app: AppState, label: String, main: String, trail: String) -> AnyElement {
    let form = app.form;
    let label = mono_words(app, label);
    let row = view().flex_row().items_center();
    let content = cells(app, vec![label], main, trail);
    match form {
        Form::Terminal => row
            .min_h(term::rows(1.0))
            .gap(term::cells(1.0))
            .child(term::space(1.0))
            .children(content)
            .into_element(),
        Form::Desktop | Form::Phone => row
            .min_h(px(ROW))
            .gap(px(8.0))
            .px(px(4.0))
            .children(content)
            .into_element(),
    }
}

/// An id, a time or a run's name in the mono font, never cut.
pub(crate) fn mono_words(app: AppState, words: String) -> AnyElement {
    let words = text(words)
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(Token::Ink));
    match app.form {
        Form::Terminal => words.into_element(),
        Form::Desktop | Form::Phone => words
            .font_family(FONT_MONO)
            .text_size(px(12.0))
            .into_element(),
    }
}

/// A child or a dependency, which opens its page (`open-task-<id>`; `task-<id>` is the list's
/// row, which the inspector stands beside).
pub(crate) fn linked(app: AppState, linked: &Linked) -> AnyElement {
    let mut lead: Vec<AnyElement> = linked
        .status
        .map(|status| status_mark(app, status))
        .into_iter()
        .collect();
    lead.push(mono_words(app, linked.id.to_string()));
    let live = linked.open();
    link_row(
        app,
        format!("open-task-{}", linked.id),
        format!("{} {}, {}", linked.id, linked.title, linked.note),
        lead,
        linked.title.clone(),
        linked.note.clone(),
        move || app.open_live(live.clone()),
    )
}

/// A run, which opens its session's page (`btn-open-reviewer`, `btn-open-run-<run>`).
pub(crate) fn run(app: AppState, run: &RunLine) -> AnyElement {
    let session = crate::model::Live::Session(run.session.clone());
    link_row(
        app,
        run.test_id.clone(),
        format!("{}, {}: open its session", run.name, run.words),
        vec![mono_words(app, run.name.clone())],
        run.words.clone(),
        String::new(),
        move || app.open_live(session.clone()),
    )
}

/// An event: when, what it says and who did it.
pub(crate) fn event(app: AppState, event: &EventLine) -> AnyElement {
    plain_row(
        app,
        event.at.clone(),
        event.text.clone(),
        event.by.to_owned(),
    )
}
