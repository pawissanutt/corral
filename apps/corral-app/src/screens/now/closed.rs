//! Now's Closed section, at the bottom of the list on every form: a heading that opens and folds
//! it (its label, the daemon's count and what landed), then one served page of done and dropped
//! tasks, newest first, with its pager. Rows sit in the task rows' columns: the id, the project,
//! the title in ink-2 (history, not work), the outcome mark and status where the rail stands
//! (done the pass mark in the ok colour, dropped the faint not-applicable mark, so they differ by
//! shape too), and the date it closed. A row selects the task, read from its own detail. While
//! open, the section keeps its page loaded.

use corral_protocol::app::CLOSED_V1;
use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::glance::Stat;
use crate::model::work::{self, ClosedPress, ClosedRow, ClosedView, NowPage};
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::kit::marks::mark;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

use super::list::{Disclosure, disclosure_head, revealing};
use super::parts::{press, pressable, quiet};
use super::rows::{
    self, ACTOR, AGE, COST, DELTA, DESK_GAP, DESK_PAD, Fit, ID, PHONE_PAD, PROJECT, RAIL, blank,
    column, figures, optional, pressable_line, words,
};

/// The test id of a Closed control.
pub(super) fn press_id(press: &ClosedPress) -> &'static str {
    match press {
        ClosedPress::Toggle => "btn-closed",
        ClosedPress::Newest => "btn-closed-newest",
        ClosedPress::Older(_) => "btn-closed-older",
        ClosedPress::Retry => "btn-closed-retry",
    }
}

/// The Closed section in the form's look once the model offers it (a complete board, even an
/// empty one). It fetches the open page when it is missing or stale; selecting a row restyles
/// it in place, keeping every row (and its focus).
pub(crate) fn closed_section(app: AppState, page: Memo<NowPage>) -> AnyElement {
    let offered = computed(move || page.with(|page| page.closed.is_some()));
    dynamic(move || {
        offered
            .get()
            .then(|| untracked(|| offered_section(app, page)))
    })
}

/// The section with no row selected: what the rows are, apart from which one is; with the
/// landed count its heading shows.
#[derive(Debug, Clone, PartialEq)]
struct Shape {
    closed: Option<ClosedView>,
    landed: Option<Stat>,
}

fn offered_section(app: AppState, page: Memo<NowPage>) -> AnyElement {
    keep_page(app);
    let form = app.form;
    let shape = computed(move || {
        page.with(|page| {
            let mut closed = page.closed.clone();
            if let Some(closed) = &mut closed {
                for row in &mut closed.rows {
                    row.selected = false;
                }
            }
            Shape {
                closed,
                landed: page.closed_landed.clone(),
            }
        })
    });
    let width = signal(0.0_f32);
    let fit = computed(move || rows::fit(form, width.get()));
    view()
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .child(dynamic(move || {
            shape.with(|shape| {
                shape
                    .closed
                    .as_ref()
                    .map(|closed| section(app, closed, shape.landed.as_ref(), page, fit))
            })
        }))
        .into_element()
}

/// Fetches the page the open section shows when it is absent or went stale: on opening, on
/// paging, when the capability arrives after the owner opened it, or after the cache let the
/// page go. A failed fetch waits for the owner's Retry; nothing loops. The page is pinned in
/// the client's cache while shown; letting go (folding, paging, or the list going) drops it
/// there, so board changes stop refetching a page nobody reads. Requests and pins go to the
/// source the section mounted on (`AppState::scope`).
fn keep_page(app: AppState) {
    let app = untracked(|| app.scope());
    let pinned = stored(None::<DetailRef>);
    effect(move || {
        let target = app
            .work
            .with(|state| state.closed.expanded.then(|| state.closed.target()));
        if pinned.with(|pinned| *pinned != target) {
            let was = pinned.replace(target.clone());
            untracked(|| {
                if let Some(gone) = was {
                    app.watch_detail(gone, false);
                }
                if let Some(new) = &target {
                    app.watch_detail(new.clone(), true);
                }
            });
        }
        let Some(target) = target else {
            return;
        };
        if !app.capable(CLOSED_V1) {
            return;
        }
        let wanted = app.with_detail(&target, |cache| {
            cache.is_none_or(|cache| cache.stale && !cache.loading && cache.error.is_none())
        });
        if wanted {
            untracked(|| app.load(target));
        }
    });
    // The owner may already be letting go of its values when this runs.
    cleanup(move || {
        if let Some(gone) = pinned.try_with(Option::take).flatten() {
            app.watch_detail(gone, false);
        }
    });
}

fn is_selected(page: &NowPage, id: &Id) -> bool {
    page.closed
        .as_ref()
        .is_some_and(|closed| closed.rows.iter().any(|row| row.selected && row.id == *id))
}

fn section(
    app: AppState,
    closed: &ClosedView,
    landed: Option<&Stat>,
    page: Memo<NowPage>,
    fit: Memo<Fit>,
) -> AnyElement {
    let action = closed.toggle.action.clone();
    let hint = closed.toggle.label.to_string();
    let stats: Vec<_> = landed.cloned().into_iter().collect();
    let head = disclosure_head(
        app,
        Disclosure {
            test_id: press_id(&ClosedPress::Toggle),
            label: closed.title,
            count: closed.total.map(|total| total.to_string()),
            stats: &stats,
            hint: &hint,
            open: closed.expanded,
        },
        move || {
            if let Some(action) = &action {
                press(app, action.clone());
            }
        },
    );
    let mut out = vec![head];
    if closed.expanded {
        if let Some(note) = &closed.note {
            out.push(quiet(app, note.clone()));
        }
        if !closed.rows.is_empty() {
            out.push(
                view()
                    .role(AccessibilityRole::List)
                    .label(closed.toggle.label.to_string())
                    .children(
                        closed
                            .rows
                            .iter()
                            .map(|row| closed_row(app, row, page, fit))
                            .collect::<Vec<_>>(),
                    )
                    .into_element(),
            );
        }
        if !closed.pager.is_empty() {
            out.push(pager(app, &closed.pager));
        }
    }
    view().children(out).into_element()
}

/// One closed task in the task rows' line; like the board's rows, it scrolls into view when the
/// list keys select it.
fn closed_row(app: AppState, row: &ClosedRow, page: Memo<NowPage>, fit: Memo<Fit>) -> AnyElement {
    let form = app.form;
    let id = row.id.clone();
    let selected = computed(move || page.with(|page| is_selected(page, &id)));
    let label = match &row.project_chip {
        Some(project) => format!("{}, {project}", row.label),
        None => row.label.clone(),
    };
    let content = match form {
        Form::Phone => vec![phone_lines(app, row)],
        Form::Desktop | Form::Terminal => line_cells(app, row, fit),
    };
    let line = pressable_line(
        app,
        label,
        format!("task-{}", row.id),
        selected,
        row.action.clone(),
        content,
        true,
    );
    let line = match form {
        Form::Desktop => line
            .px(px(DESK_PAD))
            .border_b_1()
            .border_b_color(color(Token::Rule)),
        Form::Phone => line
            .px(px(PHONE_PAD))
            .py(px(10.0))
            .border_b_1()
            .border_b_color(color(Token::Rule)),
        Form::Terminal => line,
    };
    revealing(selected, line.into_element())
}

/// The outcome mark and status words, cut to fit.
fn outcome(app: AppState, row: &ClosedRow) -> View {
    view()
        .flex_1()
        .min_w(px(0.0))
        .flex_row()
        .items_center()
        .gap(if app.form == Form::Terminal {
            term::cells(1.0)
        } else {
            px(6.0)
        })
        .child(mark(app, row.mark, row.hue))
        .child(
            words(app.form, row.status.clone(), 12.5, color(Token::Ink2))
                .flex_1()
                .min_w(px(0.0)),
        )
}

/// Desktop and terminal: the changed and actor columns left empty, id, project, title, the
/// outcome where the rail stands, the date where age and cost stand.
fn line_cells(app: AppState, row: &ClosedRow, fit: Memo<Fit>) -> Vec<AnyElement> {
    let form = app.form;
    let date = (AGE.0 + COST.0 + DESK_GAP, AGE.1 + COST.1 + 1.0);
    let mut out = vec![
        blank(form, DELTA),
        blank(form, ACTOR),
        column(
            form,
            ID,
            false,
            figures(form, row.id.to_string(), 12.0, Token::Ink3),
        )
        .into_element(),
    ];
    if let Some(project) = row.project_chip.clone() {
        out.push(optional(
            form,
            PROJECT,
            false,
            move || fit.get().project,
            words(form, project, 12.0, color(Token::Ink3)),
        ));
    }
    out.push(
        words(form, row.title.clone(), text_size(form), color(Token::Ink2))
            .flex_1()
            .min_w(px(0.0))
            .into_element(),
    );
    out.push(column(form, RAIL, false, outcome(app, row)).into_element());
    out.push(optional(
        form,
        date,
        true,
        move || fit.get().age,
        figures(form, row.at.clone(), 12.0, Token::Ink3),
    ));
    out
}

/// The phone's two lines: id, title and date; then the outcome and the project.
fn phone_lines(app: AppState, row: &ClosedRow) -> AnyElement {
    let form = app.form;
    let first = view()
        .flex_row()
        .items_baseline()
        .gap(px(8.0))
        .child(figures(form, row.id.to_string(), 13.0, Token::Ink3).flex_shrink_0())
        .child(
            text(row.title.clone())
                .flex_1()
                .min_w(px(0.0))
                .number_of_lines(2u32)
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(text_size(form)))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color(Token::Ink2)),
        )
        .child(figures(form, row.at.clone(), 12.5, Token::Ink3).flex_shrink_0());
    let second = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(outcome(app, row))
        .children(
            row.project_chip
                .clone()
                .map(|project| words(form, project, 13.0, color(Token::Ink3)).flex_shrink_0()),
        );
    view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(4.0))
        .child(first)
        .child(second)
        .into_element()
}

/// Newest, Older or Retry in a row: 8 pt apart on desktop, 10 on a phone, two blank Cells apart
/// in a terminal.
fn pager(app: AppState, buttons: &[work::Button]) -> AnyElement {
    let row = view().flex_row().flex_wrap().items_center();
    let row = match app.form {
        Form::Desktop => row
            .gap(px(8.0))
            .px(px(DESK_PAD))
            .py(px(8.0))
            .border_b_1()
            .border_b_color(color(Token::Rule)),
        Form::Phone => row.gap(px(10.0)).px(px(PHONE_PAD)).py(px(12.0)),
        Form::Terminal => row.px(term::cells(1.0)),
    };
    let mut parts = Vec::with_capacity(buttons.len() * 2);
    for (at, button) in buttons.iter().enumerate() {
        if at > 0 && app.form == Form::Terminal {
            parts.push(term::space(2.0));
        }
        parts.push(pressable(app, button, false));
    }
    row.children(parts).into_element()
}
