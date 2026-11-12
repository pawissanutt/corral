//! Rows of lists: the task row of each form, the group header over a run of task rows, and a
//! generic selectable row other lists fill.
//!
//! Desktop rows are one line in the mock's fixed columns (`.row`: Δ · actor · id · title · rail
//! · now · age · cost), 30 pt high. Phone rows are two lines (`.prow`: id · title · rail, then now
//! · age · cost), at least 60 pt high with a hairline under each. A terminal row is one line of
//! whole Cells led by a one-Cell selection bar; NSH bands the selected, hovered or focused row
//! itself, so the row draws no fill of its own there.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::board::{Actor, TaskLine};
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::kit::marks::{actor, delta, hue_ink, rail};
use crate::ui::term::{cells, hrule};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, row_height, text_size};

const RING: f32 = 2.0;
/// The phone row's least height (`.prow`).
const PHONE_ROW: f32 = 60.0;

/// Which of the task row's optional columns a row holds. The delta, actor mark, full id, title
/// and rail always show; as the row narrows the now-phrase goes first, then age, then cost, each
/// hidden whole rather than squeezed, so the title keeps at least [`TITLE_POINTS`] (desktop) or
/// [`TITLE_CELLS`] (terminal). A list's column head uses the same answer for its names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskColumns {
    pub now: bool,
    pub age: bool,
    pub cost: bool,
}

/// The least room the desktop title keeps before an optional column takes room from it.
pub const TITLE_POINTS: f32 = 120.0;
/// The least room the terminal title keeps, in Cells.
pub const TITLE_CELLS: f32 = 12.0;

/// Desktop columns in points (`.row`): leading padding 16 and trailing 14, then delta 10, actor 14,
/// id 50, the title, rail 64, now 150, age 34, cost 46, 8 apart.
const DESK_PAD: f32 = 16.0 + 14.0;
const DESK_GAP: f32 = 8.0;
const DESK_DELTA: f32 = 10.0;
const DESK_ACTOR: f32 = 14.0;
const DESK_ID: f32 = 50.0;
const DESK_RAIL: f32 = 64.0;
const DESK_NOW: f32 = 150.0;
const DESK_AGE: f32 = 34.0;
const DESK_COST: f32 = 46.0;
/// Terminal columns in Cells: the selection bar, delta, actor 1 each, id 6 (five characters and
/// room to stay whole), the title, rail 7, now 22, age 3, cost 5, a Cell apart.
const TERM_ID: f32 = 6.0;
const TERM_RAIL: f32 = 7.0;
const TERM_NOW: f32 = 22.0;
const TERM_AGE: f32 = 3.0;
const TERM_COST: f32 = 5.0;

/// The optional columns a row `width` wide holds: points on desktop and phone, whole Cells in a
/// terminal. Breakpoints (each column with its gap): desktop cost from 374 pt, age from 416 pt,
/// now from 574 pt; terminal cost from 39 Cells, age from 43, now from 66. A width of 0 (not yet
/// measured) holds every column. The phone's two-line row shows them all and ignores this.
pub fn task_columns(form: Form, width: f32) -> TaskColumns {
    if width <= 0.0 || form == Form::Phone {
        return TaskColumns {
            now: true,
            age: true,
            cost: true,
        };
    }
    let (base, gap, [cost, age, now]) = match form {
        Form::Terminal => (
            // bar, delta, actor, id, title, rail: five Cells between six columns.
            1.0 + 1.0 + 1.0 + TERM_ID + TITLE_CELLS + TERM_RAIL + 5.0,
            1.0,
            [TERM_COST, TERM_AGE, TERM_NOW],
        ),
        Form::Desktop | Form::Phone => (
            DESK_PAD
                + DESK_DELTA
                + DESK_ACTOR
                + DESK_ID
                + TITLE_POINTS
                + DESK_RAIL
                + 4.0 * DESK_GAP,
            DESK_GAP,
            [DESK_COST, DESK_AGE, DESK_NOW],
        ),
    };
    let mut room = width - base;
    let mut take = |column: f32| {
        let fits = room >= column + gap;
        if fits {
            room -= column + gap;
        }
        fits
    };
    // Cost goes last, so it is taken first; each needs the one before it.
    let cost = take(cost);
    let age = cost && take(age);
    let now = age && take(now);
    TaskColumns { now, age, cost }
}

/// The task row for `line`, selected while `selected` says so; a press calls `on_press`. The row
/// draws only what `line` holds, whichever board it came from. One element for screen readers (a
/// button with its selected state, labelled with the whole row, hidden columns included).
/// Desktop: one 30 pt line in fixed columns: the delta when changed, the actor dot, the mono id,
/// the title cut to fit, the rail, the now-phrase in its hue (needs you bold orange, held and
/// stale amber), age and cost right-aligned, cost amber above $20; selected is the sel fill with a
/// 1 pt ink bar at the leading edge. Phone: two lines: dot, id, bold title and rail; then the
/// delta, the now-phrase in its hue and `age · cost`. Terminal: one line of Cells: selection bar,
/// delta, actor glyph, id, title, rail glyphs, now, age, cost. Desktop and terminal rows measure
/// their own width and hide the now-phrase, then age, then cost as they narrow
/// ([`task_columns`]).
pub fn task_row(
    app: AppState,
    line: TaskLine,
    selected: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
    test_id: impl Into<String>,
) -> AnyElement {
    let form = app.form;
    let width = signal(0.0_f32);
    let columns = computed(move || task_columns(form, in_units(form, width.get())));
    let label = line.label();
    let content = match form {
        Form::Desktop => desktop_cells(app, line, columns),
        Form::Phone => phone_lines(app, line),
        Form::Terminal => terminal_cells(app, line, columns),
    };
    let measure = (form != Form::Phone).then_some(width);
    shell(
        app,
        label.into(),
        test_id.into(),
        selected,
        on_press,
        content,
        measure,
    )
}

/// A measured width in points as [`task_columns`] takes it: whole Cells in a terminal.
fn in_units(form: Form, points: f32) -> f32 {
    match form {
        Form::Terminal => (points / cells(1.0).0).floor(),
        Form::Desktop | Form::Phone => points,
    }
}

/// The colour of the task's now-phrase.
fn now_ink(line: &TaskLine) -> AdaptiveColor {
    match line.now_hue {
        Hue::Neutral => color(Token::Ink2),
        hue => hue_ink(hue),
    }
}

/// A one-line run of text in the mono font, for ids, ages and costs.
fn mono_cell(content: impl Into<SharedString>, size: f32, ink: AdaptiveColor) -> Text {
    text(content.into())
        .number_of_lines(1u32)
        .font_family(FONT_MONO)
        .tabular_nums()
        .text_size(px(size))
        .text_color(ink)
}

/// A fixed-width column of the desktop row.
fn column(width: f32, child: impl IntoElement) -> AnyElement {
    view()
        .w(px(width))
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .child(child)
        .into_element()
}

/// A column that gives way: laid out while `show` holds, out of the row (its gap with it) and
/// hidden from screen readers otherwise. `right` aligns its content to its trailing edge.
fn optional(
    width: Points,
    right: bool,
    show: impl Fn() -> bool + Copy + 'static,
    child: impl IntoElement,
) -> AnyElement {
    view()
        .w(width)
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .when(right, |cell| cell.justify_end())
        .style_when(move || !show(), |s| s.hidden())
        .accessibility_elements_hidden(move || !show())
        .child(child)
        .into_element()
}

/// `10px 14px 50px minmax(0, 1fr) 64px 150px 34px 46px`, 8 pt apart; now, age and cost as
/// `columns` holds them.
fn desktop_cells(app: AppState, line: TaskLine, columns: Memo<TaskColumns>) -> Vec<AnyElement> {
    let leading = if line.changed {
        delta(app)
    } else {
        view().into_element()
    };
    let id_ink = color(if line.actor == Actor::Queued {
        Token::Ink4
    } else {
        Token::Ink3
    });
    let title_ink = color(if line.quiet() {
        Token::Ink2
    } else {
        Token::Ink
    });
    let needs_you = line.now_hue == Hue::You;
    let cost_ink = color(if line.cost_high {
        Token::Held
    } else {
        Token::Ink3
    });
    let now_ink = now_ink(&line);
    vec![
        column(DESK_DELTA, leading),
        column(DESK_ACTOR, actor(app, line.actor)),
        column(DESK_ID, mono_cell(line.id, 11.5, id_ink)),
        text(line.title)
            .flex_1()
            .min_w(px(0.0))
            .truncate()
            .font_family(FONT_TEXT)
            .text_size(px(text_size(app.form)))
            .text_color(title_ink)
            .into_element(),
        column(DESK_RAIL, rail(app, line.rail)),
        optional(
            px(DESK_NOW),
            false,
            move || columns.get().now,
            text(line.now)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .text_color(now_ink)
                .when(needs_you, |t| t.font_weight(FontWeight::SEMIBOLD)),
        ),
        optional(
            px(DESK_AGE),
            true,
            move || columns.get().age,
            mono_cell(line.age, 11.0, color(Token::Ink3)),
        ),
        optional(
            px(DESK_COST),
            true,
            move || columns.get().cost,
            mono_cell(line.cost, 11.0, cost_ink),
        ),
    ]
}

/// Two lines: `id · title · rail` / `Δ now · age · cost`, the second under the title.
fn phone_lines(app: AppState, line: TaskLine) -> Vec<AnyElement> {
    let quieter = line.actor == Actor::Queued;
    let now_ink = now_ink(&line);
    let tail = line.meta();
    let first = view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .child(column(12.0, actor(app, line.actor)))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_baseline()
                .gap(px(8.0))
                .child(mono_cell(line.id, 13.0, color(Token::Ink3)).flex_shrink_0())
                .child(
                    text(line.title)
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .font_family(FONT_TEXT)
                        .text_size(px(text_size(app.form)))
                        .font_weight(if quieter {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::SEMIBOLD
                        })
                        .text_color(color(if quieter { Token::Ink2 } else { Token::Ink })),
                ),
        )
        .child(rail(app, line.rail));
    let second = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        // Under the title: past the 12 pt dot column and its 10 pt gap.
        .pl(px(22.0))
        .when(line.changed, |second| second.child(delta(app)))
        .child(
            text(line.now)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(13.5))
                .text_color(now_ink),
        )
        .child(mono_cell(tail, 12.0, color(Token::Ink3)).flex_shrink_0());
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

/// One line of Cells: Δ, actor, id (6), title, rail (7), then now (22), age (3) and cost (5) as
/// `columns` holds them.
fn terminal_cells(app: AppState, line: TaskLine, columns: Memo<TaskColumns>) -> Vec<AnyElement> {
    let leading = if line.changed {
        delta(app)
    } else {
        text(" ").into_element()
    };
    let words = |content: String, ink: Token| text(content).truncate().text_color(color(ink));
    let now_ink = match line.now_hue {
        Hue::Neutral => color(Token::Ink),
        hue => hue_ink(hue),
    };
    let needs_you = line.now_hue == Hue::You;
    let cost_ink = if line.cost_high {
        Token::Held
    } else {
        Token::Ink3
    };
    vec![
        leading,
        actor(app, line.actor),
        view()
            .w(cells(TERM_ID))
            .flex_shrink_0()
            .child(words(line.id.to_owned(), Token::Ink3))
            .into_element(),
        text(line.title)
            .flex_1()
            .min_w(px(0.0))
            .truncate()
            .text_color(color(Token::Ink))
            .into_element(),
        view()
            .w(cells(TERM_RAIL))
            .flex_shrink_0()
            .child(rail(app, line.rail))
            .into_element(),
        optional(
            cells(TERM_NOW),
            false,
            move || columns.get().now,
            text(line.now)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_color(now_ink)
                .when(needs_you, |t| t.font_weight(FontWeight::BOLD)),
        ),
        optional(
            cells(TERM_AGE),
            true,
            move || columns.get().age,
            words(line.age, Token::Ink3),
        ),
        optional(
            cells(TERM_COST),
            true,
            move || columns.get().cost,
            words(line.cost, cost_ink),
        ),
    ]
}

/// The header over a run of task rows (`.grp` on desktop, `.ph-sec` on a phone): the label in
/// the hue, the count in mono ink-3, then on desktop a hairline to the trailing edge and the
/// optional note in ink-4 ("gating · reviewing · landing"). Weights follow the mock: needs you
/// bold, corral and worker semibold, held semibold, [`Hue::Neutral`] (queued, parked) medium in
/// ink-2. Desktop 12 pt with 12 pt above and 4 pt below, 16 pt in from the edge; phone 13 pt bold
/// with 18 pt above, the note right-aligned in 12.5 pt ink-3 and no line; terminal the label bold
/// in its hue, the count, a joined `─` rule and the note. The label and count never shrink; a
/// narrow row cuts the note with an ellipsis first. A header for screen readers.
pub fn group_header(
    app: AppState,
    label: impl Into<SharedString>,
    count: Option<u32>,
    hue: Hue,
    note: Option<&'static str>,
) -> AnyElement {
    let form = app.form;
    let ink = match hue {
        Hue::Neutral => color(if form == Form::Terminal {
            Token::Ink
        } else {
            Token::Ink2
        }),
        _ => hue_ink(hue),
    };
    let weight = match (form, hue) {
        (Form::Phone, _) | (_, Hue::You) => FontWeight::BOLD,
        (_, Hue::Neutral) => FontWeight::MEDIUM,
        _ => FontWeight::SEMIBOLD,
    };
    let caption = text(label.into())
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(ink)
        .font_weight(if form == Form::Terminal {
            FontWeight::BOLD
        } else {
            weight
        });
    let figures = count.map(|n| text(n.to_string()).number_of_lines(1u32).flex_shrink_0());
    let row = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .role(AccessibilityRole::Header);
    match form {
        Form::Terminal => row
            .child(caption)
            .children(figures.map(|t| t.text_color(color(Token::Ink3))))
            .child(view().flex_1().min_w(px(0.0)).child(hrule()))
            .children(note.map(|note| {
                text(note)
                    .truncate()
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .text_color(color(Token::Ink3))
            }))
            .into_element(),
        Form::Desktop => row
            .pt(px(12.0))
            .pb(px(4.0))
            // The task rows' own edges, so the label stands over the delta column.
            .pl(px(16.0))
            .pr(px(14.0))
            .child(caption.font_family(FONT_TEXT).text_size(px(12.0)))
            .children(figures.map(|t| {
                t.font_family(FONT_MONO)
                    .text_size(px(11.0))
                    .text_color(color(Token::Ink3))
            }))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .h(px(1.0))
                    .bg(color(Token::Rule)),
            )
            .children(note.map(|note| {
                text(note)
                    .truncate()
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .font_family(FONT_TEXT)
                    .text_size(px(11.0))
                    .text_color(color(Token::Ink4))
            }))
            .into_element(),
        Form::Phone => row
            .pt(px(18.0))
            .pb(px(6.0))
            .px(px(18.0))
            .child(caption.font_family(FONT_TEXT).text_size(px(13.0)))
            .children(figures.map(|t| {
                t.font_family(FONT_MONO)
                    .text_size(px(12.0))
                    .text_color(color(Token::Ink3))
            }))
            .child(view().flex_1().min_w(px(0.0)))
            .children(note.map(|note| {
                text(note)
                    .truncate()
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .font_family(FONT_TEXT)
                    .text_size(px(12.5))
                    .text_color(color(Token::Ink3))
            }))
            .into_element(),
    }
}

/// A selectable, pressable list row the caller fills with `content`, laid out in a row: a button
/// for screen readers named `label`, with its selected state. Desktop: 30 pt high, 16 pt in on the
/// leading side and 14 pt on the trailing, children 8 pt apart, hover fill, selected the sel fill
/// with a 1 pt ink bar at the leading edge. Phone: at least 60 pt high, 10 by 18 pt padding,
/// 10 pt apart, a hairline under it, a pressed fill. Terminal: one row of Cells led by a
/// one-Cell bar on the selected row (`▌`), children a Cell apart; NSH bands it. `content` sizes
/// its own children (give the one that should give way `flex_1().min_w(0)`).
pub fn list_row(
    app: AppState,
    label: impl Into<SharedString>,
    test_id: impl Into<String>,
    selected: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
    content: Vec<AnyElement>,
) -> AnyElement {
    shell(
        app,
        label.into(),
        test_id.into(),
        selected,
        on_press,
        content,
        None,
    )
}

/// The body of [`list_row`] and [`task_row`]. With `measure`, the row records its own width
/// there after each layout, for the columns that give way.
fn shell(
    app: AppState,
    label: SharedString,
    test_id: String,
    selected: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
    content: Vec<AnyElement>,
    measure: Option<Signal<f32>>,
) -> AnyElement {
    let form = app.form;
    let selected = computed(selected);
    let row = view()
        .relative()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .accessibility_state(move || AccessibilityState {
            selected: selected.get(),
            ..AccessibilityState::default()
        })
        .test_id(&test_id)
        .on_press(move || on_press())
        .when_some(measure, |row, width| {
            row.on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        });
    let row = match form {
        Form::Desktop => row
            .h(px(row_height(form)))
            .pl(px(16.0))
            .pr(px(14.0))
            .gap(px(8.0))
            .style_when(selected, |s| s.bg(color(Token::Sel)))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-RING))
            })
            .child(
                view()
                    .absolute()
                    .left(px(0.0))
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px(1.0))
                    .bg(color(Token::Ink))
                    .opacity(move || if selected.get() { 1.0_f32 } else { 0.0_f32 }),
            ),
        Form::Phone => row
            .min_h(px(PHONE_ROW.max(row_height(form))))
            .py(px(10.0))
            .px(px(18.0))
            .gap(px(10.0))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .style_when(selected, |s| s.bg(color(Token::Sel)))
            .active(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-RING))
            }),
        // A column of its own, so the rows line up whether one is selected or not.
        Form::Terminal => row.h(px(row_height(form))).gap(px(8.0)).child(
            view()
                .w(cells(1.0))
                .flex_shrink_0()
                .accessibility_elements_hidden(true)
                .child(
                    text(move || if selected.get() { "▌" } else { " " })
                        .text_color(color(Token::Ink)),
                ),
        ),
    };
    row.children(content).into_element()
}

#[cfg(test)]
mod tests {
    use super::{TaskColumns, task_columns};
    use crate::model::Form;

    #[test]
    fn a_narrowing_row_drops_now_then_age_then_cost_and_never_brings_one_back() {
        for form in [Form::Desktop, Form::Terminal] {
            let mut last = TaskColumns {
                now: true,
                age: true,
                cost: true,
            };
            let widths: Vec<f32> = (1..=120).rev().map(|step| step as f32 * 8.0).collect();
            for width in widths {
                let width = if form == Form::Terminal {
                    width / 8.0
                } else {
                    width
                };
                let now = task_columns(form, width);
                // An optional column never shows while one that goes after it is hidden.
                assert!(!now.now || now.age, "{form:?} {width}");
                assert!(!now.age || now.cost, "{form:?} {width}");
                // Narrowing never brings a column back.
                assert!(
                    !(now.now && !last.now) && !(now.age && !last.age) && !(now.cost && !last.cost)
                );
                last = now;
            }
            assert_eq!(
                last,
                TaskColumns {
                    now: false,
                    age: false,
                    cost: false
                }
            );
        }
    }
}
