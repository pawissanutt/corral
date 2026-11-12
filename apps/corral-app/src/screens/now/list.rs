//! "Who acts next": the grouped list of the model (`work::GroupedList`) in each form's rows. On
//! desktop and phone a task row is the kit's; the plan and PM-ask rows, the collapsed queued line
//! and the parked block are this list's own, laid in the same columns. The list measures its width
//! once, and its own head and rows give way as the kit's task rows do (`rows::task_columns`).

use nsh_core::prelude::*;

use crate::model::work::{self, ExtraRow, GroupedList, ParkedRow, Row};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::{actor, delta, dot, hue_ink};
use crate::ui::kit::notice::skeleton;
use crate::ui::kit::rows::{TaskColumns, group_header, list_row, task_row};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

use super::cells;
use super::parts::{act, action_id, model_button, question_open};

const RING: f32 = 2.0;

/// The desktop and terminal list: the stale board's `as of` chip, the column head, the groups.
pub(super) fn list(app: AppState) -> AnyElement {
    let source = computed(move || {
        let env = app.env();
        let open = question_open(app, env);
        app.work
            .with(|state| work::grouped_list_with_question(state, env, open))
    });
    let as_of = computed(move || work::status(app.env()).as_of);
    let label = untracked(|| source.with(|list| list.label));
    let (width, columns) = cells::measured(app.form);
    view()
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

/// The Goals panel: the model's Goals control (its press explains the filter).
pub(super) fn goals(app: AppState) -> AnyElement {
    let goals = untracked(|| {
        let env = app.env();
        app.work.with(|state| work::workspace(state, env).goals)
    });
    view()
        .flex_row()
        .px(px(if app.form == Form::Terminal {
            8.0
        } else {
            16.0
        }))
        .py(px(if app.form == Form::Terminal {
            0.0
        } else {
            14.0
        }))
        .child(model_button(app, &goals, "btn-goals".into(), false))
        .into_element()
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

fn is_selected(source: Memo<GroupedList>, id: &'static str) -> bool {
    source.with(|list| {
        list.groups
            .iter()
            .flat_map(|group| group.rows.iter())
            .any(|row| matches!(row, Row::Task(task) if task.id == id && task.selected))
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
                    let (id, action) = (task.id, task.action);
                    task_row(
                        app,
                        id,
                        move || is_selected(source, id),
                        move || act(app, action),
                        format!("task-{id}"),
                    )
                }
                Row::Extra(extra) => extra_row(app, extra, columns),
                Row::More(button) => {
                    let label = button.label.to_string();
                    let action = button.action;
                    quiet_row(app, label, action.map(action_id), move || {
                        if let Some(action) = action {
                            act(app, action);
                        }
                    })
                }
                Row::Parked(parked) => parked_row(app, parked, columns),
            });
        }
    }
    view().children(out).into_element()
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

/// A plan accept or the sample PM ask, in the task columns: the changed mark, the needs-you dot,
/// the id, the title, the kind where the rail sits, the note in its hue, the age. The phone's two
/// lines: id and title, then the note and the age.
fn extra_row(app: AppState, row: &ExtraRow, columns: Memo<TaskColumns>) -> AnyElement {
    let form = app.form;
    let label = [
        row.display_id,
        row.extra.title,
        row.kind_label,
        row.extra.note,
        row.age,
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    if form == Form::Terminal {
        return cells::extra_row(app, row, label, columns);
    }
    let size = text_size(form);
    let note_ink = match row.note_hue {
        Hue::Neutral => color(Token::Ink2),
        hue => hue_ink(hue),
    };
    let leading = if row.changed { delta(app) } else { blank() };
    let content = match form {
        Form::Phone => vec![
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(10.0))
                        .child(fixed(12.0, false, dot(app, Hue::You)))
                        .child(line(form, row.display_id, 13.0, color(Token::Ink3), true))
                        .child(
                            line(form, row.extra.title, size, color(Token::Ink), false)
                                .flex_1()
                                .min_w(px(0.0))
                                .font_weight(FontWeight::SEMIBOLD),
                        ),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .pl(px(22.0))
                        .child(
                            line(form, row.extra.note, 13.5, note_ink, false)
                                .flex_1()
                                .min_w(px(0.0)),
                        )
                        .child(line(form, row.age, 12.0, color(Token::Ink3), true)),
                )
                .into_element(),
        ],
        Form::Desktop | Form::Terminal => vec![
            fixed(10.0, false, leading),
            fixed(14.0, false, dot(app, Hue::You)),
            fixed(
                50.0,
                false,
                line(form, row.display_id, 11.5, color(Token::Ink3), true).into_element(),
            ),
            line(form, row.extra.title, size, color(Token::Ink), false)
                .flex_1()
                .min_w(px(0.0))
                .into_element(),
            fixed(
                64.0,
                false,
                line(form, row.kind_label, 11.5, color(Token::Ink3), false).into_element(),
            ),
            give_way(
                150.0,
                false,
                move || columns.get().now,
                line(form, row.extra.note, 12.0, note_ink, false).into_element(),
            ),
            give_way(
                34.0,
                true,
                move || columns.get().age,
                line(form, row.age, 11.0, color(Token::Ink3), true).into_element(),
            ),
            give_way(46.0, true, move || columns.get().cost, blank()),
        ],
    };
    let action = row.action;
    list_row(
        app,
        label,
        action_id(action),
        || false,
        move || act(app, action),
        content,
    )
}

/// The parked block (`.r-parked`): the held count where the id sits, the projects in the title's
/// place, "held by you" where the now-phrase sits; on the phone one quiet line.
fn parked_row(app: AppState, row: &ParkedRow, columns: Memo<TaskColumns>) -> AnyElement {
    let form = app.form;
    let action = row.action;
    if form == Form::Phone {
        return quiet_row(app, row.title.clone(), Some(action_id(action)), move || {
            act(app, action)
        });
    }
    let label = format!("{}, {}, {}", row.summary.n, row.title, row.now);
    if form == Form::Terminal {
        return cells::parked_row(app, row, label, columns);
    }
    let content = vec![
        fixed(10.0, false, blank()),
        fixed(14.0, false, actor(app, crate::model::board::Actor::Parked)),
        fixed(
            50.0,
            false,
            line(
                form,
                row.summary.n.to_string(),
                11.5,
                color(Token::Ink3),
                true,
            )
            .into_element(),
        ),
        line(
            form,
            row.title.clone(),
            text_size(form),
            color(Token::Ink2),
            false,
        )
        .flex_1()
        .min_w(px(0.0))
        .into_element(),
        fixed(64.0, false, blank()),
        give_way(
            150.0,
            false,
            move || columns.get().now,
            line(form, row.now, 12.0, color(Token::Ink3), false).into_element(),
        ),
        give_way(34.0, true, move || columns.get().age, blank()),
        give_way(46.0, true, move || columns.get().cost, blank()),
    ];
    list_row(
        app,
        label,
        action_id(action),
        || false,
        move || act(app, action),
        content,
    )
}

/// A quiet pressable line (`.more`): the collapsed queued tasks; on the phone also the parked
/// block. Desktop 26 pt, under the titles (48 pt in), a chevron after the words; phone at least
/// 48 pt with a hairline under it; terminal one row of Cells.
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
