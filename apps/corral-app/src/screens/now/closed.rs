//! Now's Closed section, under the list's groups: a heading that opens and folds it, then one
//! served page of done and dropped tasks, newest first, with its pager. Rows sit in the task
//! rows' columns: the mark where the actor dot is (done the pass mark in the ok colour, dropped
//! the faint not-applicable mark, so they differ by shape too), the id, the title, the status
//! with the plan and goal it served, and the date it closed. A row opens the task from its own
//! detail. While open, the section keeps its page loaded.

use corral_protocol::app::CLOSED_V1;
use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::work::{self, ClosedPress, ClosedRow, ClosedView};
use crate::model::{Form, Hue, Id};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::mark;
use crate::ui::kit::rows::{TaskColumns, group_header, list_row, titled};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

use super::cells;
use super::list::revealing;
use super::parts::{press, pressable, quiet, scope};

const RING: f32 = 2.0;
/// The group heading's own vertical padding (`rows::group_header`), so the disclosure chevron
/// centres on its label.
const DESK_HEAD: (f32, f32) = (12.0, 4.0);
const PHONE_HEAD: (f32, f32) = (18.0, 6.0);
/// The task rows' leading columns: delta, actor and id.
const DESK_DELTA: f32 = 10.0;
const DESK_MARK: f32 = 14.0;
const DESK_ID: f32 = 50.0;
const DESK_STATUS: f32 = 150.0;
/// Wide enough for `Nov 12` in 11 pt mono.
const DESK_DATE: f32 = 46.0;
const TERM_ID: f32 = 6.0;
const TERM_STATUS: f32 = 22.0;
const TERM_DATE: f32 = 6.0;

/// The test id of a Closed control.
pub(super) fn press_id(press: &ClosedPress) -> &'static str {
    match press {
        ClosedPress::Toggle => "btn-closed",
        ClosedPress::Newest => "btn-closed-newest",
        ClosedPress::Older(_) => "btn-closed-older",
        ClosedPress::Retry => "btn-closed-retry",
    }
}

/// The Closed section in the form's look, for the bottom of the Now list on every form, once
/// the model offers it (`work::closed_offered`: a complete board, even an empty one). It reads
/// `work::closed_view_filtered` in Now's project scope itself and fetches the open page when it
/// is missing or stale;
/// selecting a row restyles it in place, keeping every row (and its focus).
pub(super) fn closed_section(app: AppState) -> AnyElement {
    let offered = computed(move || app.feed.with(work::closed_offered));
    dynamic(move || offered.get().then(|| untracked(|| offered_section(app))))
}

fn offered_section(app: AppState) -> AnyElement {
    keep_page(app);
    let source = computed(move || {
        let selection = app.selection();
        let (filter, _) = scope(app);
        app.feed.with(|feed| {
            app.work.with(|state| {
                work::closed_view_filtered(state, feed, &selection, app.form, &filter)
            })
        })
    });
    let shape = computed(move || source.with(unselected));
    let (width, columns) = cells::measured(app.form);
    view()
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .child(dynamic(move || {
            shape.with(|closed| section(app, closed, source, columns))
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

/// The section with no row selected: what the rows are, apart from which one is.
fn unselected(closed: &ClosedView) -> ClosedView {
    let mut closed = closed.clone();
    for row in &mut closed.rows {
        row.selected = false;
    }
    closed
}

fn is_selected(source: Memo<ClosedView>, id: &Id) -> bool {
    source.with(|closed| closed.rows.iter().any(|row| row.selected && row.id == *id))
}

fn section(
    app: AppState,
    closed: &ClosedView,
    source: Memo<ClosedView>,
    columns: Memo<TaskColumns>,
) -> AnyElement {
    let mut out = vec![toggle(app, closed)];
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
                            .map(|row| closed_row(app, row, source, columns))
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

/// The heading as a disclosure: the list's group heading (label and the daemon's count), then a
/// chevron that turns down while the section is open (the terminal's own mark leads the label
/// instead). One button for screen readers, named with the count and saying whether it is
/// expanded.
fn toggle(app: AppState, closed: &ClosedView) -> AnyElement {
    let form = app.form;
    let open = closed.expanded;
    let count = closed
        .total
        .map(|total| u32::try_from(total).unwrap_or(u32::MAX));
    let heading = view()
        .flex_1()
        .min_w(px(0.0))
        .accessibility_elements_hidden(true)
        .child(group_header(app, closed.title, count, Hue::Neutral, None));
    let chevron = |(top, bottom): (f32, f32)| {
        view()
            .flex_shrink_0()
            .pt(px(top))
            .pb(px(bottom))
            .child(
                view()
                    .rotate(move || deg(if open { 90.0 } else { 0.0 }))
                    .child(icon(Icon::Chevron, 12.0, color(Token::Ink3))),
            )
            .into_element()
    };
    let action = closed.toggle.action.clone();
    let row = view()
        .flex_row()
        .items_center()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(closed.toggle.label.to_string())
        .accessibility_state(move || AccessibilityState {
            expanded: Some(open),
            ..AccessibilityState::default()
        })
        .test_id(press_id(&ClosedPress::Toggle))
        .on_press(move || {
            if let Some(action) = &action {
                press(app, action.clone());
            }
        })
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-RING))
        })
        .child(heading);
    match form {
        Form::Desktop => row
            .pr(px(14.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .child(chevron(DESK_HEAD)),
        Form::Phone => row
            .min_h(px(48.0))
            .pr(px(18.0))
            .active(|s| s.bg(color(Token::Hover)))
            .child(chevron(PHONE_HEAD)),
        // The terminal paints a disclosure's ▸ / ▾ before its label itself, from `expanded`.
        Form::Terminal => row,
    }
    .into_element()
}

/// One closed task in the kit's list row: selected while it is the task shown, a press opens it;
/// like the board's rows, it scrolls into view when the list keys select it.
fn closed_row(
    app: AppState,
    row: &ClosedRow,
    source: Memo<ClosedView>,
    columns: Memo<TaskColumns>,
) -> AnyElement {
    let content = match app.form {
        Form::Desktop => desktop_cells(app, row, columns),
        Form::Phone => phone_lines(app, row),
        Form::Terminal => terminal_cells(app, row, columns),
    };
    let id = row.id.clone();
    let selected = {
        let id = id.clone();
        computed(move || is_selected(source, &id))
    };
    let action = row.action.clone();
    let label = match &row.project_chip {
        Some(project) => format!("{}, {project}", row.label),
        None => row.label.clone(),
    };
    let row = list_row(
        app,
        label,
        format!("task-{}", row.id),
        move || is_selected(source, &id),
        move || press(app, action.clone()),
        content,
    );
    revealing(selected, row)
}

/// A one-line run of mono figures: ids and dates.
fn mono(content: impl Into<SharedString>, size: f32) -> Text {
    text(content.into())
        .number_of_lines(1u32)
        .font_family(FONT_MONO)
        .tabular_nums()
        .text_size(px(size))
        .text_color(color(Token::Ink3))
}

/// A fixed column `width` wide; `right` aligns its content to its trailing edge.
fn column(width: Points, right: bool, child: impl IntoElement) -> View {
    view()
        .w(width)
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .when(right, |cell| cell.justify_end())
        .child(child)
}

/// A column that gives way with the list's now-phrase column: out of the row and hidden from
/// screen readers (the row's label still says it) while the list is too narrow.
fn status_column(width: Points, columns: Memo<TaskColumns>, child: impl IntoElement) -> AnyElement {
    let show = move || columns.get().now;
    cells::optional(column(width, false, child), show)
        .accessibility_elements_hidden(move || !show())
        .into_element()
}

/// `10 14 50 1fr 150 46`, 8 pt apart: the task rows' leading columns, the title in ink-2 (a
/// closed task is history, not work), the status that gives way, the date right-aligned.
fn desktop_cells(app: AppState, row: &ClosedRow, columns: Memo<TaskColumns>) -> Vec<AnyElement> {
    vec![
        column(px(DESK_DELTA), false, view()).into_element(),
        column(px(DESK_MARK), false, mark(app, row.mark, row.hue)).into_element(),
        column(px(DESK_ID), false, mono(row.id.to_string(), 11.5)).into_element(),
        titled(
            app,
            text(row.title.clone())
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(text_size(app.form)))
                .text_color(color(Token::Ink2)),
            row.project_chip.clone(),
        ),
        status_column(
            px(DESK_STATUS),
            columns,
            text(row.status.clone())
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .text_color(color(Token::Ink3)),
        ),
        column(px(DESK_DATE), true, mono(row.at.clone(), 11.0)).into_element(),
    ]
}

/// The task row's two lines: `mark id title` / `status · date`, the second under the title.
fn phone_lines(app: AppState, row: &ClosedRow) -> Vec<AnyElement> {
    let first = view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .child(column(px(12.0), false, mark(app, row.mark, row.hue)))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_baseline()
                .gap(px(8.0))
                .child(mono(row.id.to_string(), 13.0).flex_shrink_0())
                .child(
                    text(row.title.clone())
                        .flex_1()
                        .min_w(px(0.0))
                        .number_of_lines(2u32)
                        .font_family(FONT_TEXT)
                        .text_size(px(text_size(app.form)))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color(Token::Ink2)),
                ),
        );
    let second = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        // Under the title: past the 12 pt mark column and its 10 pt gap.
        .pl(px(22.0))
        .children(
            row.project_chip
                .clone()
                .map(|project| chip(app, project, Hue::Neutral)),
        )
        .child(
            text(row.status.clone())
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(13.5))
                .text_color(color(Token::Ink3)),
        )
        .child(mono(row.at.clone(), 12.0).flex_shrink_0());
    vec![
        view()
            .flex_1()
            .min_w(px(0.0))
            .gap(px(2.0))
            .child(first)
            .child(second)
            .into_element(),
    ]
}

/// One line of Cells: Δ, the mark, id (6), title, status (22, giving way), date (6).
fn terminal_cells(app: AppState, row: &ClosedRow, columns: Memo<TaskColumns>) -> Vec<AnyElement> {
    let words = |content: String, ink: Token| text(content).truncate().text_color(color(ink));
    vec![
        text(" ").into_element(),
        mark(app, row.mark, row.hue),
        column(
            term::cells(TERM_ID),
            false,
            words(row.id.to_string(), Token::Ink3),
        )
        .into_element(),
        titled(
            app,
            words(row.title.clone(), Token::Ink2),
            row.project_chip.clone(),
        ),
        status_column(
            term::cells(TERM_STATUS),
            columns,
            words(row.status.clone(), Token::Ink3)
                .flex_1()
                .min_w(px(0.0)),
        ),
        column(
            term::cells(TERM_DATE),
            true,
            words(row.at.clone(), Token::Ink3),
        )
        .into_element(),
    ]
}

/// Newest, Older or Retry in a row: 8 pt apart on desktop, 10 on a phone, two blank Cells apart
/// in a terminal, as the other terminal button rows lay them.
fn pager(app: AppState, buttons: &[work::Button]) -> AnyElement {
    let row = view().flex_row().flex_wrap().items_center();
    let row = match app.form {
        Form::Desktop => row.gap(px(8.0)).px(px(16.0)).py(px(8.0)),
        Form::Phone => row.gap(px(10.0)).px(px(18.0)).py(px(12.0)),
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
