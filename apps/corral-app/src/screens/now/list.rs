//! "Who acts next": the grouped list of the model (`work::GroupedList`) in each form's rows, built
//! from the feed's board. On desktop and phone a task row is the kit's; the collapsed queued line
//! is this list's own, laid in the same columns. The list measures its width once, and its own
//! head and rows give way as the kit's task rows do (`rows::task_columns`). It is the
//! destination's keyboard list (`AppState::list_keys`), and a row the keyboard selects scrolls
//! into its panel's view.

use nsh_core::prelude::*;

use crate::model::work::{self, GroupedList, Row};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::chip::chip;
use crate::ui::kit::notice::{notice, skeleton};
use crate::ui::kit::rows::{TaskColumns, group_header, list_row, task_row};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::cells;
use super::parts::{press, press_id, pressable, quiet};
use super::verbs::{row_record, with_menu};

const RING: f32 = 2.0;

/// The desktop and terminal list: corrald's word on a board it cannot keep current, with its
/// Retry, the stale board's `as of` chip, the column head, the groups.
pub(super) fn list(app: AppState) -> AnyElement {
    let form = app.form;
    let source = computed(move || {
        let selection = app.selection();
        app.feed.with(|feed| {
            app.work
                .with(|state| work::daemon_list(state, feed, &selection, form))
        })
    });
    let board = computed(move || app.feed.with(|feed| work::daemon_status(feed).banner));
    let as_of = computed(move || app.feed.with(|feed| work::daemon_status(feed).as_of));
    let label = untracked(|| source.with(|list| list.label));
    let (width, columns) = cells::measured(app.form);
    view()
        .element_ref(&app.list_keys)
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .pt(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
        .pb(px(if app.form == Form::Terminal {
            0.0
        } else {
            14.0
        }))
        .role(AccessibilityRole::List)
        .label(label)
        .child(dynamic(move || {
            board
                .get()
                .map(|banner| untracked(|| board_notice(app, &banner)))
        }))
        .child(dynamic(move || {
            as_of.get().map(|stamp| {
                view()
                    .flex_row()
                    .px(px(16.0))
                    .pb(px(4.0))
                    .child(chip(app, stamp, Hue::Held))
                    .into_element()
            })
        }))
        .child(rows(app, source, columns))
        .into_element()
}

/// corrald's notice that the board is out of date or refreshing, with its Retry (the only
/// action the daemon's board notice carries; `btn-retry-board`), above the rows it qualifies.
fn board_notice(app: AppState, banner: &work::Banner) -> AnyElement {
    let retry = banner
        .actions
        .first()
        .map(|button| pressable(app, button, false));
    let room = match app.form {
        Form::Terminal => view(),
        Form::Desktop | Form::Phone => view().px(px(16.0)).pb(px(8.0)),
    };
    room.child(notice(app, banner.hue, &banner.text, retry))
        .into_element()
}

/// The Goals panel's title and list label.
pub(super) const GOALS_LABEL: &str = "Goals";

/// The Goals panel: the goals the board's tasks serve, each narrowing the list to it.
pub(super) fn goals(app: AppState) -> AnyElement {
    let rows = computed(move || {
        app.feed
            .with(|feed| app.work.with(|state| work::daemon_goals(state, feed)))
    });
    dynamic(move || {
        rows.with(|rows| {
            if rows.is_empty() {
                return quiet(app, work::NO_GOALS);
            }
            view()
                .py(px(if app.form == Form::Terminal { 0.0 } else { 4.0 }))
                .role(AccessibilityRole::List)
                .label(GOALS_LABEL)
                .children(
                    rows.iter()
                        .enumerate()
                        .map(|(index, row)| goal_row(app, row, index))
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
    })
}

/// One goal the board serves: its words, then how many tasks serve it in mono figures; the
/// goal the list is narrowed to carries the list's selection. A press narrows the list to it.
fn goal_row(app: AppState, row: &work::GoalRow, index: usize) -> AnyElement {
    let form = app.form;
    let selected = row.selected;
    let action = row.button.action.clone();
    let content = vec![
        line(
            form,
            row.button.label.to_string(),
            if form == Form::Phone { 14.0 } else { 12.5 },
            color(Token::Ink),
            false,
        )
        .flex_1()
        .min_w(px(0.0))
        .into_element(),
        line(form, row.count.to_string(), 11.0, color(Token::Ink3), true).into_element(),
    ];
    list_row(
        app,
        format!("{}, {} tasks", row.button.label, row.count),
        format!("btn-goal-{index}"),
        move || selected,
        move || {
            if let Some(action) = &action {
                press(app, action.clone());
            }
        },
        content,
    )
}

/// The list's rows for `source`, built again only when its shape changes: selecting another task
/// restyles the two rows concerned and keeps every row (and its focus) in place. `columns` says
/// which optional columns the list's width holds, as the kit's task rows decide for themselves.
pub(super) fn rows(
    app: AppState,
    source: Memo<GroupedList>,
    columns: Memo<TaskColumns>,
) -> AnyElement {
    let shape = computed(move || source.with(unselected));
    dynamic(move || shape.with(|list| groups(app, list, source, columns)))
}

/// The list with no row selected: what the rows are, apart from which one is.
fn unselected(list: &GroupedList) -> GroupedList {
    let mut list = list.clone();
    for row in list
        .groups
        .iter_mut()
        .flat_map(|group| group.rows.iter_mut())
    {
        if let Row::Task(task) = row {
            task.selected = false;
        }
    }
    list
}

fn is_selected(source: Memo<GroupedList>, id: &str) -> bool {
    source.with(|list| {
        list.groups
            .iter()
            .flat_map(|group| group.rows.iter())
            .any(|row| matches!(row, Row::Task(task) if task.line.id == id && task.selected))
    })
}

fn groups(
    app: AppState,
    list: &GroupedList,
    source: Memo<GroupedList>,
    columns: Memo<TaskColumns>,
) -> AnyElement {
    if list.loading_rows != 0 {
        return loading(app, list.loading_rows);
    }
    if let Some(notice) = list.notice {
        return quiet(app, notice);
    }
    let mut out: Vec<AnyElement> = Vec::new();
    if app.form == Form::Terminal {
        out.push(cells::head(list, columns));
    } else if !list.columns.is_empty() {
        out.push(column_head(list, columns));
    }
    for group in &list.groups {
        out.push(group_header(
            app,
            group.label,
            Some(group.count),
            group.hue,
            group.detail,
        ));
        for row in &group.rows {
            out.push(match row {
                Row::Task(task) => {
                    let id = task.line.id.clone();
                    let action = task.action.clone();
                    let record = row_record(&action);
                    let test_id = format!("task-{id}");
                    let selected = {
                        let id = id.clone();
                        computed(move || is_selected(source, &id))
                    };
                    let row = task_row(
                        app,
                        task.line.clone(),
                        move || is_selected(source, &id),
                        move || press(app, action.clone()),
                        test_id,
                    );
                    // A daemon task's row carries its commands: its context menu on desktop
                    // and terminal, the action sheet a held finger opens on the phone.
                    let row = match record {
                        Some(record) => with_menu(app, record, row),
                        None => row,
                    };
                    revealing(selected, row)
                }
                Row::More(button) => {
                    let label = button.label.to_string();
                    let action = button.action.clone();
                    quiet_row(app, label, action.as_ref().map(press_id), move || {
                        if let Some(action) = &action {
                            press(app, action.clone());
                        }
                    })
                }
            });
        }
    }
    view().children(out).into_element()
}

/// `row`, scrolled into its panel's view whenever it becomes the selected one, so a selection
/// the keyboard moves never leaves the viewport. A row already in view stays where it is.
fn revealing(selected: Memo<bool>, row: AnyElement) -> AnyElement {
    let handle = element_ref();
    effect(move || {
        if selected.get() {
            after_answer(move || {
                handle.reveal(RevealOptions::default().vertical(ScrollAlignment::Nearest));
            });
        }
    });
    view().element_ref(&handle).child(row).into_element()
}

/// Placeholder rows while the projection's detail loads (`.skel` in the list's columns).
fn loading(app: AppState, count: usize) -> AnyElement {
    let form = app.form;
    let rows = (0..count).map(|index| {
        // The mock varies each title bar's length so the rows read as text.
        let title = 0.6 + ((index * 13) % 35) as f32 / 100.0;
        match form {
            Form::Phone => view()
                .px(px(18.0))
                .py(px(7.0))
                .child(skeleton(app, &[(0.7, 14.0), (0.4, 10.0)]))
                .into_element(),
            Form::Terminal => view()
                .px(term::cells(1.0))
                .child(skeleton(app, &[(title, 16.0)]))
                .into_element(),
            Form::Desktop => view()
                .flex_row()
                .gap(px(10.0))
                .px(px(16.0))
                .py(px(9.0))
                .child(view().w(px(50.0)).child(skeleton(app, &[(1.0, 10.0)])))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(skeleton(app, &[(title, 10.0)])),
                )
                .child(view().w(px(64.0)).child(skeleton(app, &[(1.0, 10.0)])))
                .child(view().w(px(120.0)).child(skeleton(app, &[(1.0, 10.0)])))
                .into_element(),
        }
    });
    view()
        .label("Loading")
        .children(rows.collect::<Vec<_>>())
        .into_element()
}

/// A fixed-width column of a desktop or phone row.
fn fixed(points: f32, right: bool, child: AnyElement) -> AnyElement {
    view()
        .w(px(points))
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .when(right, |cell| cell.justify_end())
        .child(child)
        .into_element()
}

/// A desktop column that gives way: hidden whole, for sight and screen readers, while `show`
/// does not hold.
fn give_way(
    points: f32,
    right: bool,
    show: impl Fn() -> bool + Copy + 'static,
    child: AnyElement,
) -> AnyElement {
    let cell = view()
        .w(px(points))
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .when(right, |cell| cell.justify_end())
        .accessibility_elements_hidden(move || !show())
        .child(child);
    cells::optional(cell, show).into_element()
}

/// One line of text in the list's look: mono figures, or the text face at `size`.
fn line(
    form: Form,
    content: impl Into<SharedString>,
    size: f32,
    ink: AdaptiveColor,
    mono: bool,
) -> Text {
    let words = text(content.into())
        .number_of_lines(1u32)
        .truncate()
        .text_color(ink);
    match (form, mono) {
        (Form::Terminal, _) => words,
        (_, true) => words
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(size)),
        (_, false) => words.font_family(FONT_TEXT).text_size(px(size)),
    }
}

fn blank() -> AnyElement {
    view().into_element()
}

/// The desktop column head (`.colhead`): the model's column names over id, task, rail, now, age,
/// cost, in 10.5 pt spaced capitals, faint; the rail's names its stages for screen readers.
fn column_head(list: &GroupedList, columns: Memo<TaskColumns>) -> AnyElement {
    let name = |index: usize| -> Text {
        text(list.columns.get(index).copied().unwrap_or_default())
            .number_of_lines(1u32)
            .uppercase()
            .font_family(FONT_TEXT)
            .text_size(px(10.5))
            .letter_spacing(px(10.5 * 0.06))
            .text_color(color(Token::Ink4))
    };
    let rail = name(2).label(list.rail_title);
    view()
        .flex_row()
        .items_center()
        .accessibility_elements_hidden(true)
        .gap(px(8.0))
        .pl(px(16.0))
        .pr(px(14.0))
        .h(px(22.0))
        .child(fixed(10.0, false, blank()))
        .child(fixed(14.0, false, blank()))
        .child(fixed(50.0, false, name(0).into_element()))
        .child(view().flex_1().min_w(px(0.0)).child(name(1)))
        .child(fixed(64.0, false, rail.into_element()))
        .child(give_way(
            150.0,
            false,
            move || columns.get().now,
            name(3).into_element(),
        ))
        .child(give_way(
            34.0,
            true,
            move || columns.get().age,
            name(4).into_element(),
        ))
        .child(give_way(
            46.0,
            true,
            move || columns.get().cost,
            name(5).into_element(),
        ))
        .into_element()
}

/// A quiet pressable line (`.more`): the collapsed queued tasks. Desktop 26 pt, under the titles
/// (48 pt in), a chevron after the words; phone at least 48 pt with a hairline under it;
/// terminal one row of Cells.
fn quiet_row(
    app: AppState,
    label: String,
    test_id: Option<String>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let words = line(
        form,
        label.clone(),
        if form == Form::Phone { 14.0 } else { 12.0 },
        color(if form == Form::Terminal {
            Token::Ink
        } else {
            Token::Ink3
        }),
        false,
    );
    let row = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .flex_shrink_0()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .when_some(test_id, |row, id| row.test_id(&id))
        .on_press(move || on_press());
    let row = match form {
        Form::Desktop => row
            .h(px(26.0))
            .pl(px(48.0))
            .pr(px(16.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-RING))
            })
            .child(words)
            .child(icon(Icon::Chevron, 12.0, color(Token::Ink3))),
        Form::Phone => row
            .min_h(px(48.0))
            .px(px(18.0))
            .py(px(10.0))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .active(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-RING))
            })
            .child(words.flex_1().min_w(px(0.0))),
        // Under the titles: past the bar, Δ, actor and id columns and their gaps.
        Form::Terminal => row.h(px(16.0)).pl(term::cells(13.0)).child(words),
    };
    row.into_element()
}
